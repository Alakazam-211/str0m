//! A DTLS client that answers the server's CertificateRequest with an empty Certificate
//! must not complete the handshake: the server expects the remote fingerprint from the SDP,
//! and str0m checks that fingerprint against the certificate the peer presents.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc, RtcError};

mod common;
use common::{Peer, init_crypto_default, init_log};

const CONTENT_HANDSHAKE: u8 = 22;
const HANDSHAKE_CERTIFICATE: u8 = 11;
const HANDSHAKE_CERTIFICATE_VERIFY: u8 = 15;

/// Rewrite a DTLS 1.2 datagram: an epoch 0 Certificate record becomes a Certificate with an
/// empty certificate list (same message_seq), and a CertificateVerify record is dropped, as a
/// client without a certificate would send its flight. Returns `None` if the datagram has no
/// Certificate record.
fn empty_client_certificate(datagram: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(datagram.len());
    let mut rewrote = false;
    let mut i = 0usize;
    while i + 13 <= datagram.len() {
        let len = u16::from_be_bytes([datagram[i + 11], datagram[i + 12]]) as usize;
        let end = (i + 13 + len).min(datagram.len());
        let record = &datagram[i..end];
        let epoch = u16::from_be_bytes([record[3], record[4]]);
        let msg_type = record.get(13).copied();
        if record[0] == CONTENT_HANDSHAKE && epoch == 0 && msg_type == Some(HANDSHAKE_CERTIFICATE) {
            // Record header (13) + handshake header (12) + an empty certificate_list (3).
            let mut empty = record[..25].to_vec();
            empty[11..13].copy_from_slice(&15u16.to_be_bytes());
            empty[14..17].copy_from_slice(&[0, 0, 3]); // handshake length
            empty[19..22].copy_from_slice(&[0, 0, 0]); // fragment offset
            empty[22..25].copy_from_slice(&[0, 0, 3]); // fragment length
            empty.extend_from_slice(&[0, 0, 0]);
            out.extend_from_slice(&empty);
            rewrote = true;
        } else if record[0] == CONTENT_HANDSHAKE
            && epoch == 0
            && msg_type == Some(HANDSHAKE_CERTIFICATE_VERIFY)
        {
            // A client without a certificate sends no CertificateVerify.
        } else {
            out.extend_from_slice(record);
        }
        i = end;
    }
    rewrote.then_some(out)
}

struct Packet {
    source: std::net::SocketAddr,
    destination: std::net::SocketAddr,
    contents: Vec<u8>,
}

/// Poll `rtc` until it times out; queue its transmits and note whether it connected.
fn poll(rtc: &mut Rtc, out: &mut Vec<Packet>, connected: &mut bool) -> Result<(), RtcError> {
    loop {
        match rtc.poll_output()? {
            Output::Timeout(_) => return Ok(()),
            Output::Transmit(t) => out.push(Packet {
                source: t.source,
                destination: t.destination,
                contents: t.contents.to_vec(),
            }),
            Output::Event(Event::Connected) => *connected = true,
            Output::Event(_) => {}
        }
    }
}

fn receive(rtc: &mut Rtc, now: Instant, p: &Packet) -> Result<(), RtcError> {
    rtc.handle_input(Input::Receive(
        now,
        Receive {
            proto: Protocol::Udp,
            source: p.source,
            destination: p.destination,
            contents: (&p.contents[..]).try_into()?,
        },
    ))
}

#[test]
fn dtls12_empty_client_certificate_fails_the_handshake() {
    init_log();
    init_crypto_default();

    let start = Instant::now();
    let mut builder_l = Rtc::builder();
    if let Some(crypto) = Peer::Left.crypto_provider() {
        builder_l = builder_l.set_crypto_provider(crypto);
    }
    let mut builder_r = Rtc::builder();
    if let Some(crypto) = Peer::Right.crypto_provider() {
        builder_r = builder_r.set_crypto_provider(crypto);
    }
    let mut l = builder_l.build(start);
    let mut r = builder_r.build(start);

    let host_l = Candidate::host((Ipv4Addr::new(1, 1, 1, 1), 1000).into(), "udp").unwrap();
    let host_r = Candidate::host((Ipv4Addr::new(2, 2, 2, 2), 2000).into(), "udp").unwrap();
    l.add_local_candidate(host_l.clone());
    l.add_remote_candidate(host_r.clone());
    r.add_local_candidate(host_r);
    r.add_remote_candidate(host_l);

    // Each side expects the other's real certificate fingerprint.
    let finger_l = l.direct_api().local_dtls_fingerprint().clone();
    let finger_r = r.direct_api().local_dtls_fingerprint().clone();
    l.direct_api().set_remote_fingerprint(finger_r);
    r.direct_api().set_remote_fingerprint(finger_l);

    let creds_l = l.direct_api().local_ice_credentials();
    let creds_r = r.direct_api().local_ice_credentials();
    l.direct_api().set_remote_ice_credentials(creds_r);
    r.direct_api().set_remote_ice_credentials(creds_l);
    l.direct_api().set_ice_controlling(true);
    r.direct_api().set_ice_controlling(false);

    // L is the DTLS client, R the server that requires a client certificate.
    l.direct_api().start_dtls(true).unwrap();
    r.direct_api().start_dtls(false).unwrap();

    let mut l_connected = false;
    let mut r_connected = false;
    let mut rewrote = false;
    let mut r_error: Option<String> = None;
    let mut to_l = Vec::new();
    let mut to_r = Vec::new();

    let mut now = start;
    while now - start < Duration::from_secs(10) && r_error.is_none() {
        l.handle_input(Input::Timeout(now)).unwrap();
        r.handle_input(Input::Timeout(now)).unwrap();
        poll(&mut l, &mut to_r, &mut l_connected).unwrap();
        poll(&mut r, &mut to_l, &mut r_connected).unwrap();

        for p in to_r.drain(..) {
            let p = match p.contents.first() {
                // DTLS (RFC 7983 demultiplexing).
                Some(20..=63) => match empty_client_certificate(&p.contents) {
                    Some(contents) => {
                        rewrote = true;
                        Packet { contents, ..p }
                    }
                    None => p,
                },
                _ => p,
            };
            let result = receive(&mut r, now, &p).and_then(|_| {
                let mut out = Vec::new();
                let res = poll(&mut r, &mut out, &mut r_connected);
                to_l.extend(out);
                res
            });
            if let Err(e) = result {
                r_error = Some(e.to_string());
                break;
            }
        }
        for p in to_l.drain(..) {
            // L may fail once R aborts; only R's outcome matters here.
            if receive(&mut l, now, &p).is_ok() {
                let _ = poll(&mut l, &mut to_r, &mut l_connected);
            }
        }

        now += Duration::from_millis(10);
    }

    assert!(rewrote, "the client's Certificate should have been replaced");
    assert!(
        !r_connected && !r.is_connected(),
        "the server must not complete a handshake with an empty client Certificate"
    );
    let err = r_error.expect("the server should fail the handshake with an error");
    assert!(
        err.contains("no client certificate received"),
        "the server should reject the empty client Certificate, got: {err}"
    );
}
