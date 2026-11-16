//! The remote certificate fingerprint used for DTLS is the BUNDLE-tag media
//! section's, which overrides a session-level default.
#![cfg(any(
    feature = "aws-lc-rs",
    feature = "rust-crypto",
    feature = "openssl",
    feature = "openssl-dimpl",
    feature = "wincrypto",
    feature = "apple-crypto",
))]

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use str0m::{Candidate, RtcConfig, RtcError};
use tracing::info_span;

mod common;
use common::{TestRtc, init_crypto_default, init_log, progress};

#[test]
fn media_transport_fingerprint_overrides_session_in_negotiation() -> Result<(), RtcError> {
    init_log();
    init_crypto_default();
    let now = Instant::now();
    let mut l = TestRtc::new_with_rtc(info_span!("L"), RtcConfig::new().build(now));
    let mut r = TestRtc::new_with_rtc(info_span!("R"), RtcConfig::new().build(now));
    l.add_local_candidate(
        Candidate::host((Ipv4Addr::new(1, 1, 1, 1), 1000).into(), "udp").unwrap(),
    )
    .unwrap();
    r.add_local_candidate(
        Candidate::host((Ipv4Addr::new(2, 2, 2, 2), 2000).into(), "udp").unwrap(),
    )
    .unwrap();
    let mut wrong = l.direct_api().local_dtls_fingerprint().clone();
    wrong.bytes[0] ^= 0xff;
    let mut change = l.sdp_api();
    change.add_channel("test".into());
    let (offer, pending) = change.apply().unwrap();
    let source = offer.to_sdp_string();
    let (session, media) = source.split_once("\r\nm=").unwrap();
    let rewritten = format!("{session}\r\na=fingerprint:{wrong}\r\nm={media}");
    let offer = str0m::change::SdpOffer::from_sdp_string(&rewritten).unwrap();
    let answer = r.sdp_api().accept_offer(offer).unwrap();
    l.sdp_api().accept_answer(pending, answer).unwrap();
    while !(l.is_connected() && r.is_connected()) {
        assert!(
            l.duration() < Duration::from_secs(5),
            "DTLS did not connect"
        );
        progress(&mut l, &mut r)?;
    }
    Ok(())
}
