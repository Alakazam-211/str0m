//! TWCC feedback from the remote peer, fed through `Rtc::handle_input`.
//!
//! Negotiate and connect real peers, then install known SRTP keys on the sender so the test can
//! encrypt hand-built TWCC reports as the remote would and inject them.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use str0m::_internal_test_exports::TestSrtp;
use str0m::crypto::dtls::{KeyingMaterial, SrtpProfile};
use str0m::media::{Direction, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::rtp::RawPacket;
use str0m::rtp::rtcp::Rtcp;
use str0m::{Event, Input, Output};

mod common;
use common::{Peer, TestRtc, init_crypto_default, init_log, negotiate, progress};

const L_ADDR: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 1000);
const R_ADDR: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2)), 2000);
const STATS_INTERVAL: Duration = Duration::from_millis(100);

/// A report whose first chunk is a zero-length run, followed by a run of large deltas, must not
/// take the zero run's status for the later packets (which would read a large delta as a small
/// one and panic). The packets it reports received are received.
#[test]
fn zero_length_run_before_large_deltas() {
    let (mut l, mut srtp, sent) = sender();
    let base = sent[0];

    let chunks = [
        0x2000, // run of 0 small delta
        0x4001, // run of 1 large delta
        0x2002, // run of 2 small delta
    ];
    let deltas = [
        0x00, 0x08, // large delta 2ms
        0x04, // small delta 1ms
        0x04, // small delta 1ms
    ];
    let report = twcc_report(base, 3, &chunks, &deltas);
    inject(&mut l, &srtp.protect_rtcp(&report));

    assert_eq!(parsed_reports(&l), 1, "the report reached the session");
    assert_eq!(egress_loss_after(&mut l), Some(0.0));
}

/// A report whose first chunk is a zero-length not-received run must not report every later
/// packet lost.
#[test]
fn zero_length_not_received_run_does_not_mark_later_packets_lost() {
    let (mut l, mut srtp, sent) = sender();
    let base = sent[0];
    let n = sent.len() as u16;

    let chunks = [
        0x0000,     // run of 0 not received
        0x2000 | n, // run of n small delta
        0x0000,     // run of 0 not received
    ];
    let deltas = vec![0x50; n as usize]; // small delta 20ms each
    let report = twcc_report(base, n, &chunks, &deltas);
    inject(&mut l, &srtp.protect_rtcp(&report));

    assert_eq!(parsed_reports(&l), 1, "the report reached the session");
    assert_eq!(egress_loss_after(&mut l), Some(0.0));
}

/// A run of large deltas is two bytes per delta: a report that holds the large deltas but not
/// the small delta after them is rejected, and the complete report is applied.
#[test]
fn large_delta_run_followed_by_a_small_delta() {
    let (mut l, mut srtp, sent) = sender();
    let base = sent[0];

    let chunks = [
        0x4002, // run of 2 large delta
        0x2001, // run of 1 small delta
    ];
    let large = [
        0x00, 0x50, // large delta 20ms
        0x00, 0x50, // large delta 20ms
    ];
    // 24 bytes of body with no padding: the small delta is missing.
    let short = twcc_report(base, 3, &chunks, &large);
    assert_eq!(short.len(), 4 + 24);
    inject(&mut l, &srtp.protect_rtcp(&short));
    assert_eq!(
        parsed_reports(&l),
        0,
        "a report too short for its deltas is rejected"
    );

    let mut deltas = large.to_vec();
    deltas.push(0x50); // small delta 20ms
    let report = twcc_report(base, 3, &chunks, &deltas);
    inject(&mut l, &srtp.protect_rtcp(&report));
    assert_eq!(parsed_reports(&l), 1, "the report reached the session");
    assert_eq!(egress_loss_after(&mut l), Some(0.0));
}

/// Connect a video sender L to R, install known SRTP keys on L, then send a few packets from L
/// alone. Returns L, an SRTP context that encrypts as R does, and the TWCC sequence numbers L sent.
fn sender() -> (TestRtc, TestSrtp, Vec<u16>) {
    init_log();
    init_crypto_default();

    let configure = |config: str0m::RtcConfig| {
        config
            .set_stats_interval(Some(STATS_INTERVAL))
            .enable_raw_packets(true)
    };
    let mut l = TestRtc::new_with_config(Peer::Left, configure);
    let mut r = TestRtc::new_with_config(Peer::Right, configure);
    l.add_host_candidate(L_ADDR);
    r.add_host_candidate(R_ADDR);

    let mid: Mid = negotiate(&mut l, &mut r, |change| {
        change.add_media(MediaKind::Video, Direction::SendOnly, None, None, None)
    });
    while !l.is_connected() || !r.is_connected() {
        assert!(l.duration() < Duration::from_secs(10), "failed to connect");
        progress(&mut l, &mut r).unwrap();
    }

    // ICE and DTLS establish a real transport. Replace only the SRTP keys so the test can
    // encrypt reports as the remote. From here on L runs alone.
    let profile = SrtpProfile::AEAD_AES_128_GCM;
    let crypto = str0m::crypto::from_feature_flags();
    let material = KeyingMaterial::new(&[42; 56]);
    let srtp = TestSrtp::new(&crypto, profile, &material);
    l._set_srtp_keys(material, profile, false);
    l.last = l.last.max(r.last);
    drain(&mut l);
    l.events.clear();

    let pt = l.params_vp8().pt();
    for _ in 0..4 {
        let wallclock = l.start + l.duration();
        let time = l.duration().into();
        l.writer(mid)
            .unwrap()
            .write(pt, wallclock, time, [1_u8; 80])
            .unwrap();
        advance(&mut l, Duration::from_millis(20));
    }

    let sent: Vec<u16> = l
        .events
        .iter()
        .filter_map(|(_, e)| match e.as_raw_packet() {
            Some(RawPacket::RtpTx(header, _)) => header.ext_vals.transport_cc,
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 4, "one packet per frame: {sent:?}");
    for w in sent.windows(2) {
        assert_eq!(
            w[0].wrapping_add(1),
            w[1],
            "consecutive TWCC sequence numbers"
        );
    }
    l.events.clear();

    (l, srtp, sent)
}

/// A TWCC RTCP packet (header included), zero-padded to a multiple of 4 bytes.
fn twcc_report(base_seq: u16, status_count: u16, chunks: &[u16], deltas: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&1_u32.to_be_bytes()); // sender ssrc
    body.extend_from_slice(&2_u32.to_be_bytes()); // media ssrc
    body.extend_from_slice(&base_seq.to_be_bytes());
    body.extend_from_slice(&status_count.to_be_bytes());
    body.extend_from_slice(&[0, 0, 0]); // reference time
    body.push(0); // feedback count
    for c in chunks {
        body.extend_from_slice(&c.to_be_bytes());
    }
    body.extend_from_slice(deltas);
    while body.len() % 4 != 0 {
        body.push(0);
    }

    let words = (4 + body.len()) / 4;
    let mut packet = vec![0x80 | 15, 205];
    packet.extend_from_slice(&((words - 1) as u16).to_be_bytes());
    packet.extend_from_slice(&body);
    packet
}

fn inject(l: &mut TestRtc, packet: &[u8]) {
    let receive = Receive {
        proto: Protocol::Udp,
        source: R_ADDR,
        destination: L_ADDR,
        contents: packet.try_into().unwrap(),
    };
    let input = Input::Receive(l.last, receive);
    l.handle_input(input).unwrap();
    drain(l);
}

fn parsed_reports(l: &TestRtc) -> usize {
    l.events
        .iter()
        .filter(|(_, e)| matches!(e.as_raw_packet(), Some(RawPacket::RtcpRx(Rtcp::Twcc(_)))))
        .count()
}

/// The egress loss L reports in its first peer stats after now.
fn egress_loss_after(l: &mut TestRtc) -> Option<f32> {
    let from = l.last;
    advance(l, STATS_INTERVAL * 2);
    l.events
        .iter()
        .filter(|(t, _)| *t >= from)
        .find_map(|(_, e)| match e {
            Event::PeerStats(s) => Some(s.egress_loss_fraction),
            _ => None,
        })
        .expect("a peer stats event")
}

/// Follow the timeouts requested by `Rtc` for `duration`.
fn advance(l: &mut TestRtc, duration: Duration) {
    let end = l.last + duration;
    loop {
        let next = drain(l);
        if next > end {
            l.last = end;
            break;
        }
        l.last = next.max(l.last);
        let input = Input::Timeout(l.last);
        l.handle_input(input).unwrap();
    }
}

fn drain(l: &mut TestRtc) -> Instant {
    loop {
        match l.poll_output().unwrap() {
            Output::Event(event) => l.events.push((l.last, event)),
            Output::Transmit(_) => {}
            Output::Timeout(next) => return next,
        }
    }
}
