use std::collections::VecDeque;
use std::time::{Duration, Instant};

use netem::{NetemConfig, Probability, RandomLoss};
use str0m::format::Codec;
use str0m::media::{MediaKind, Pt};
use str0m::rtp::rtcp::Rtcp;
use str0m::rtp::{ExtensionValues, RawPacket, RtpWrite, Ssrc};
use str0m::{Event, Rtc, RtcError};

mod common;
use common::{Peer, TestRtc, connect_l_r, connect_l_r_with_rtc};
use common::{init_crypto_default, init_log, progress};

#[test]
pub fn rtp_direct_reset_ssrc() -> Result<(), RtcError> {
    init_log();
    init_crypto_default();

    let (mut l, mut r) = connect_l_r();

    let mid = "aud".into();
    let rid = "hi".into();

    // Initial SSRC
    let ssrc_tx_initial: Ssrc = 42.into();
    // New SSRC to use after reset
    let ssrc_tx_new: Ssrc = 84.into();

    l.direct_api().declare_media(mid, MediaKind::Audio);
    l.direct_api()
        .declare_stream_tx(ssrc_tx_initial, None, mid, Some(rid));

    r.direct_api()
        .declare_media(mid, MediaKind::Audio)
        .expect_rid_rx(rid);

    // Set initial timing
    let max = l.last.max(r.last);
    l.last = max;
    r.last = max;

    let params = l.params_opus();
    let ssrc = l
        .direct_api()
        .stream_tx_by_mid(mid, Some(rid))
        .unwrap()
        .ssrc();
    assert_eq!(
        ssrc, ssrc_tx_initial,
        "Initial SSRC should match what we set"
    );
    assert_eq!(params.spec().codec, Codec::Opus);
    let pt = params.pt();

    // First batch of packets with initial SSRC
    let first_batch: Vec<&[u8]> = vec![&[0x1, 0x2, 0x3, 0x4], &[0x5, 0x6, 0x7, 0x8]];

    let mut first_batch: VecDeque<_> = first_batch.into();
    let mut first_counts: Vec<u64> = vec![0, 1];
    let mut write_at = l.last + Duration::from_millis(300);

    // Send first batch with initial SSRC
    loop {
        if l.start + l.duration() > write_at {
            write_at = l.last + Duration::from_millis(300);
            if let Some(packet) = first_batch.pop_front() {
                let wallclock = l.start + l.duration();
                let mut direct = l.direct_api();
                let stream = direct.stream_tx_by_mid(mid, Some(rid)).unwrap();

                let count = first_counts.remove(0);
                let time = (count * 1000 + 47_000_000) as u32;
                let seq_no = (47_000 + count).into();

                let exts = ExtensionValues {
                    audio_level: Some(-42 - count as i8),
                    voice_activity: Some(false),
                    ..Default::default()
                };

                stream.write_rtp(RtpWrite::new(pt, seq_no, time, wallclock, packet).ext_vals(exts));
            }
        }

        progress(&mut l, &mut r)?;

        if first_batch.is_empty() && first_counts.is_empty() {
            break;
        }
    }

    // Run a bit longer to ensure first batch of packets arrive
    for _ in 0..20 {
        progress(&mut l, &mut r)?;
    }

    // Try resetting to the same SSRC (should fail and return None)
    let mut api = l.direct_api();
    let result = api.reset_stream_tx(mid, Some(rid), ssrc_tx_initial, None);
    assert!(
        result.is_none(),
        "Resetting to the same SSRC should return None"
    );

    // Reset the SSRC with None for RTX since the stream doesn't use RTX
    let mut api = l.direct_api();
    let result = api.reset_stream_tx(mid, Some(rid), ssrc_tx_new, None);
    assert!(result.is_some(), "Reset should succeed with valid new SSRC");

    // Verify the SSRC was changed
    let updated_ssrc = l
        .direct_api()
        .stream_tx_by_mid(mid, Some(rid))
        .unwrap()
        .ssrc();
    assert_eq!(
        updated_ssrc, ssrc_tx_new,
        "SSRC should be updated to new value"
    );

    // Second batch of packets with new SSRC
    let second_batch: Vec<&[u8]> = vec![&[0x9, 0xa, 0xb, 0xc], &[0xd, 0xe, 0xf, 0x10]];

    let mut second_batch: VecDeque<_> = second_batch.into();
    let mut second_counts: Vec<u64> = vec![0, 1];
    write_at = l.last + Duration::from_millis(300);

    // Send second batch with new SSRC
    loop {
        if l.start + l.duration() > write_at {
            write_at = l.last + Duration::from_millis(300);
            if let Some(packet) = second_batch.pop_front() {
                let wallclock = l.start + l.duration();
                let mut direct = l.direct_api();
                let stream = direct.stream_tx_by_mid(mid, Some(rid)).unwrap();

                let count = second_counts.remove(0);
                let time = (count * 1000 + 48_000_000) as u32;
                let seq_no = (48_000 + count).into(); // Different seq range to verify reset

                let exts = ExtensionValues {
                    audio_level: Some(-52 - count as i8),
                    voice_activity: Some(false),
                    ..Default::default()
                };

                stream.write_rtp(RtpWrite::new(pt, seq_no, time, wallclock, packet).ext_vals(exts));
            }
        }

        progress(&mut l, &mut r)?;

        if second_batch.is_empty() && second_counts.is_empty() {
            // Run a bit longer to ensure packets arrive
            for _ in 0..20 {
                progress(&mut l, &mut r)?;
            }
            break;
        }
    }

    // Collect all received media packets
    let media: Vec<_> = r
        .events
        .iter()
        .filter_map(|(_, e)| {
            if let Event::RtpPacket(v) = e {
                Some(v)
            } else {
                None
            }
        })
        .collect();

    // Should have received all 4 packets
    assert_eq!(media.len(), 4, "Should have received all 4 packets");

    // Verify that we have packets with both SSRCs
    let first_ssrc_packets: Vec<_> = media
        .iter()
        .filter(|p| p.header.ssrc == ssrc_tx_initial)
        .collect();
    let second_ssrc_packets: Vec<_> = media
        .iter()
        .filter(|p| p.header.ssrc == ssrc_tx_new)
        .collect();

    assert_eq!(
        first_ssrc_packets.len(),
        2,
        "Should have 2 packets with initial SSRC"
    );
    assert_eq!(
        second_ssrc_packets.len(),
        2,
        "Should have 2 packets with new SSRC"
    );

    // Verify specific packet properties
    // First SSRC packets
    let h0 = &first_ssrc_packets[0].header;
    let h1 = &first_ssrc_packets[1].header;

    assert_eq!(h0.sequence_number, 47000);
    assert_eq!(h1.sequence_number, 47001);

    assert_eq!(h0.timestamp, 47_000_000);
    assert_eq!(h1.timestamp, 47_001_000);

    assert_eq!(h0.ext_vals.audio_level, Some(-42));
    assert_eq!(h1.ext_vals.audio_level, Some(-43));

    // Second SSRC packets - sequence numbers should be reset
    let h2 = &second_ssrc_packets[0].header;
    let h3 = &second_ssrc_packets[1].header;

    assert_eq!(h2.sequence_number, 48000);
    assert_eq!(h3.sequence_number, 48001);

    assert_eq!(h2.timestamp, 48_000_000);
    assert_eq!(h3.timestamp, 48_001_000);

    assert_eq!(h2.ext_vals.audio_level, Some(-52));
    assert_eq!(h3.ext_vals.audio_level, Some(-53));

    // Clean up
    l.direct_api().remove_media(mid);
    r.direct_api().remove_media(mid);

    Ok(())
}

#[test]
pub fn rtp_direct_reset_ssrc_with_rtx() -> Result<(), RtcError> {
    init_log();
    init_crypto_default();

    let (mut l, mut r) = connect_l_r();

    let mid = "vid".into();
    let rid = "hi".into();

    // Initial SSRCs
    let ssrc_tx_initial: Ssrc = 100.into();
    let ssrc_rtx_initial: Ssrc = 101.into();

    // New SSRCs to use after reset
    let ssrc_tx_new: Ssrc = 200.into();
    let ssrc_rtx_new: Ssrc = 201.into();

    // Create media and stream with both main and RTX SSRCs
    l.direct_api().declare_media(mid, MediaKind::Video);
    l.direct_api()
        .declare_stream_tx(ssrc_tx_initial, Some(ssrc_rtx_initial), mid, Some(rid));

    r.direct_api()
        .declare_media(mid, MediaKind::Video)
        .expect_rid_rx(rid);

    // Set initial timing
    let max = l.last.max(r.last);
    l.last = max;
    r.last = max;

    // Verify the initial SSRCs
    {
        let mut api = l.direct_api();
        let stream = api.stream_tx_by_mid(mid, Some(rid)).unwrap();
        assert_eq!(
            stream.ssrc(),
            ssrc_tx_initial,
            "Initial SSRC should match what we set"
        );
        assert_eq!(
            stream.rtx(),
            Some(ssrc_rtx_initial),
            "Initial RTX SSRC should match what we set"
        );
    }

    // Try resetting with the same RTX SSRC (should fail)
    let mut api = l.direct_api();
    let result = api.reset_stream_tx(mid, Some(rid), ssrc_tx_new, Some(ssrc_rtx_initial));
    assert!(result.is_none(), "Reset with same RTX SSRC should fail");

    // Reset with new main and RTX SSRCs (should succeed)
    let mut api = l.direct_api();
    let result = api.reset_stream_tx(mid, Some(rid), ssrc_tx_new, Some(ssrc_rtx_new));
    assert!(
        result.is_some(),
        "Reset with new main and RTX SSRCs should succeed"
    );

    // Verify the SSRCs were changed
    {
        let mut api = l.direct_api();
        let stream = api.stream_tx_by_mid(mid, Some(rid)).unwrap();
        assert_eq!(
            stream.ssrc(),
            ssrc_tx_new,
            "SSRC should be updated to new value"
        );
        assert_eq!(
            stream.rtx(),
            Some(ssrc_rtx_new),
            "RTX SSRC should be updated to new value"
        );
    }

    // Clean up
    l.direct_api().remove_media(mid);
    r.direct_api().remove_media(mid);

    Ok(())
}

/// Writes `n` nackable packets on `ssrc` (sequence numbers from `first_seq`), one per
/// `progress`. With `lossy`, R loses 5% of what reaches it except the first and last 10.
fn write_nackable(
    l: &mut TestRtc,
    r: &mut TestRtc,
    ssrc: Ssrc,
    pt: Pt,
    first_seq: u64,
    n: u64,
    lossy: bool,
) -> Result<(), RtcError> {
    for i in 0..n {
        if lossy && i == 10 {
            r.set_netem(
                NetemConfig::new()
                    .loss(RandomLoss::new(Probability::new(0.05)))
                    .seed(42),
            );
        }
        if i + 10 == n {
            r.set_netem(NetemConfig::new());
        }
        let wallclock = l.start + l.duration();
        let seq_no = (first_seq + i).into();
        let time = ((first_seq + i) * 3000) as u32;
        l.direct_api()
            .stream_tx(&ssrc)
            .expect("the send stream is found by its SSRC")
            .write_rtp(
                RtpWrite::new(pt, seq_no, time, wallclock, [0x1, 0x2, 0x3, 0x4]).nackable(true),
            );
        progress(l, r)?;
    }
    r.set_netem(NetemConfig::new());
    Ok(())
}

fn progress_for(l: &mut TestRtc, r: &mut TestRtc, d: Duration) -> Result<(), RtcError> {
    let until = l.duration() + d;
    while l.duration() < until {
        progress(l, r)?;
    }
    Ok(())
}

/// `reset_stream_tx` changes the SSRC of a send stream, and the stream must then be found by
/// its new SSRC: incoming RTCP finds its send stream by the SSRC it names. After a reset the
/// receiver's reports, a PLI and NACKs for the new SSRC reach the stream (the NACKs are
/// answered on the new RTX SSRC), the old SSRC names nothing (a PLI for it is dropped), and no
/// RTX SSRC keys the table. (Sew issue #7: a reused send slot renews its SSRC this way, and a
/// subscriber's NACKs and PLIs for it were dropped.)
#[test]
pub fn rtp_direct_reset_ssrc_rtcp_reaches_new_ssrc() -> Result<(), RtcError> {
    init_log();
    init_crypto_default();

    let now = Instant::now();
    let mut lb = Rtc::builder()
        .set_rtp_mode(true)
        .enable_raw_packets(true)
        .set_stats_interval(Some(Duration::from_millis(500)));
    if let Some(crypto) = Peer::Left.crypto_provider() {
        lb = lb.set_crypto_provider(crypto);
    }
    let mut rb = Rtc::builder().set_rtp_mode(true).enable_raw_packets(true);
    if let Some(crypto) = Peer::Right.crypto_provider() {
        rb = rb.set_crypto_provider(crypto);
    }
    let (mut l, mut r) = connect_l_r_with_rtc(lb.build(now), rb.build(now));

    let mid = "vid".into();
    let (old, old_rtx): (Ssrc, Ssrc) = (100.into(), 101.into());
    let (new, new_rtx): (Ssrc, Ssrc) = (200.into(), 201.into());
    let feedback_from: Ssrc = 7.into();

    l.direct_api().declare_media(mid, MediaKind::Video);
    l.direct_api()
        .declare_stream_tx(old, Some(old_rtx), mid, None);
    r.direct_api().declare_media(mid, MediaKind::Video);
    r.direct_api()
        .expect_stream_rx(old, Some(old_rtx), mid, None);

    let max = l.last.max(r.last);
    l.last = max;
    r.last = max;

    let params = l.params_vp8();
    let pt = params.pt();
    let rtx_pt = params.resend().expect("VP8 has an RTX payload type");

    // Media on the old SSRC first, so the stream has state to reset.
    write_nackable(&mut l, &mut r, old, pt, 1_000, 50, false)?;
    progress_for(&mut l, &mut r, Duration::from_secs(2))?;

    let reset = l
        .direct_api()
        .reset_stream_tx(mid, None, new, Some(new_rtx))
        .map(|s| (s.ssrc(), s.rtx()));
    assert_eq!(reset, Some((new, Some(new_rtx))));
    {
        let mut api = l.direct_api();
        assert!(api.stream_tx(&old).is_none(), "the old SSRC names nothing");
        assert!(api.stream_tx(&old_rtx).is_none());
        assert!(
            api.stream_tx(&new_rtx).is_none(),
            "an RTX SSRC never keys the table"
        );
        let s = api
            .stream_tx(&new)
            .expect("the stream is found by its new SSRC");
        assert_eq!(s.rtx(), Some(new_rtx));
    }
    r.direct_api().remove_stream_rx(old);
    r.direct_api()
        .expect_stream_rx(new, Some(new_rtx), mid, None);
    let reset_at = l.last;
    l.events.clear();
    r.events.clear();

    let keyframe_requests = |l: &TestRtc| {
        l.events
            .iter()
            .filter(|(_, e)| matches!(e, Event::KeyframeRequest(_)))
            .count()
    };

    // A PLI for the old SSRC reaches no stream.
    r.direct_api().send_pli_feedback(feedback_from, old);
    progress_for(&mut l, &mut r, Duration::from_secs(1))?;
    assert_eq!(
        keyframe_requests(&l),
        0,
        "a PLI for the old SSRC is dropped"
    );

    // A PLI for the new SSRC reaches the stream.
    r.direct_api().send_pli_feedback(feedback_from, new);
    progress_for(&mut l, &mut r, Duration::from_secs(1))?;
    assert_eq!(
        keyframe_requests(&l),
        1,
        "a PLI for the new SSRC is a keyframe request"
    );

    // Media on the new SSRC through 5% loss: R's NACKs name the new SSRC, L answers them on
    // the new RTX SSRC, and every packet arrives.
    write_nackable(&mut l, &mut r, new, pt, 5_000, 300, true)?;
    progress_for(&mut l, &mut r, Duration::from_secs(5))?;

    let nacks_rx: Vec<Ssrc> = l
        .events
        .iter()
        .filter_map(|(_, e)| match e.as_raw_packet() {
            Some(RawPacket::RtcpRx(Rtcp::Nack(n))) => Some(n.ssrc),
            _ => None,
        })
        .collect();
    assert!(!nacks_rx.is_empty(), "the loss produced NACKs");
    assert!(nacks_rx.iter().all(|s| *s == new), "{nacks_rx:?}");

    let mut seqs = Vec::new();
    let mut resent = 0;
    for (_, e) in &r.events {
        let Some(RawPacket::RtpRx(h, b)) = e.as_raw_packet() else {
            continue;
        };
        if h.payload_type == rtx_pt {
            assert_eq!(h.ssrc, new_rtx, "resends go out on the new RTX SSRC");
            resent += 1;
            seqs.push(u16::from_be_bytes([b[0], b[1]]));
        } else if h.ssrc == new {
            seqs.push(h.sequence_number);
        }
    }
    assert!(resent > 0, "L answered the NACKs");
    seqs.sort_unstable();
    seqs.dedup();
    let want: Vec<u16> = (5_000..5_300).collect();
    assert_eq!(seqs, want, "every packet on the new SSRC arrived");

    // The receiver's reports for the new SSRC reach the stream: its egress stats (reset with
    // the SSRC) carry a remote report, the PLI and the NACKs.
    let stats = l
        .events
        .iter()
        .filter_map(|(at, e)| match e {
            Event::MediaEgressStats(s) if *at > reset_at => Some(s),
            _ => None,
        })
        .next_back()
        .expect("egress stats after the reset");
    assert!(
        stats.remote.is_some(),
        "a receiver report reached the stream: {stats:?}"
    );
    assert_eq!(stats.plis, 1, "{stats:?}");
    assert!(stats.nacks > 0, "{stats:?}");

    Ok(())
}
