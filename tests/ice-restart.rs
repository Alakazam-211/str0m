use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use str0m::RtcError;
use str0m::media::{Direction, MediaKind};
use str0m::{IceCreds, RtcConfig};
use tracing::info_span;

mod common;
use common::{Peer, TestRtc, init_crypto_default, init_log, progress};

#[test]
pub fn ice_restart() -> Result<(), RtcError> {
    init_log();
    init_crypto_default();

    let mut l = TestRtc::new(Peer::Left);

    let rtc = RtcConfig::new().set_ice_lite(true).build(Instant::now());
    let mut r = TestRtc::new_with_rtc(info_span!("R"), rtc);

    l.add_host_candidate((Ipv4Addr::new(1, 1, 1, 1), 1000).into());
    r.add_host_candidate((Ipv4Addr::new(2, 2, 2, 2), 2000).into());

    let (offer, pending) = l.span.in_scope(|| {
        let mut change = l.rtc.sdp_api();
        let _ = change.add_channel("My little channel".into());

        change.apply().unwrap()
    });
    println!("L Initial Offer: {}", offer);

    let answer = r.span.in_scope(|| r.rtc.sdp_api().accept_offer(offer))?;
    println!("R Initial answer: {}", answer);

    l.span
        .in_scope(|| l.rtc.sdp_api().accept_answer(pending, answer))?;

    loop {
        if l.is_connected() && r.is_connected() {
            break;
        }
        progress(&mut l, &mut r)?;
    }

    let l_creds = l._local_ice_creds();
    let r_creds = r._local_ice_creds();

    let (offer, pending) = r.span.in_scope(|| {
        let mut change = r.rtc.sdp_api();
        change.ice_restart(true);

        change.apply().expect("Should be able to apply changes")
    });
    println!("R Offer: {}", offer);

    let answer = l.span.in_scope(|| l.rtc.sdp_api().accept_offer(offer))?;
    println!("L Answer: {}", answer);
    r.span
        .in_scope(|| r.rtc.sdp_api().accept_answer(pending, answer))?;

    assert!(!l.rtc.is_connected());
    assert!(!r.rtc.is_connected());

    loop {
        if l.duration() > Duration::from_secs(10) {
            panic!("Failed to re-establish connectivity after ICE restart in 10 seconds");
        }

        if l.is_connected() && r.is_connected() {
            break;
        }

        progress(&mut l, &mut r)?;
    }

    assert_ne!(
        r_creds,
        r._local_ice_creds(),
        "After an ICE restart ICE credentials should have changed"
    );
    assert_ne!(
        l_creds,
        l._local_ice_creds(),
        "After an ICE restart ICE credentials should have changed"
    );

    Ok(())
}

fn creds(ufrag: &str, pass: &str) -> IceCreds {
    IceCreds {
        ufrag: ufrag.into(),
        pass: pass.into(),
    }
}

fn assert_only_creds(sdp: &str, c: &IceCreds) {
    let mut ufrags = 0;
    for line in sdp.lines() {
        if let Some(u) = line.strip_prefix("a=ice-ufrag:") {
            assert_eq!(u, c.ufrag, "a=ice-ufrag in {sdp}");
            ufrags += 1;
        }
        if let Some(p) = line.strip_prefix("a=ice-pwd:") {
            assert_eq!(p, c.pass, "a=ice-pwd in {sdp}");
        }
        if line.starts_with("a=candidate:") {
            let mut words = line.split(' ');
            while let Some(w) = words.next() {
                if w == "ufrag" {
                    assert_eq!(
                        words.next(),
                        Some(c.ufrag.as_str()),
                        "candidate ufrag in {sdp}"
                    );
                }
            }
        }
    }
    assert!(ufrags > 0, "no a=ice-ufrag in {sdp}");
}

#[test]
pub fn ice_restart_with_given_credentials() -> Result<(), RtcError> {
    init_log();
    init_crypto_default();

    let initial = creds("S1tN4kQ7pR2vX9bL", "c8JfT3mW6yH1dZ5qK0sA7gP2xE9nV4rU");
    let mut l = TestRtc::new(Peer::Left);
    let rtc = RtcConfig::new()
        .set_ice_lite(true)
        .set_local_ice_credentials(initial.clone())
        .build(Instant::now());
    let mut r = TestRtc::new_with_rtc(info_span!("R"), rtc);

    l.add_host_candidate((Ipv4Addr::new(1, 1, 1, 1), 1000).into());
    r.add_host_candidate((Ipv4Addr::new(2, 2, 2, 2), 2000).into());

    let (offer, pending) = l.span.in_scope(|| {
        let mut change = l.rtc.sdp_api();
        let _ = change.add_channel("chan".into());
        change.apply().unwrap()
    });
    let answer = r.span.in_scope(|| r.rtc.sdp_api().accept_offer(offer))?;
    assert_only_creds(&answer.to_sdp_string(), &initial);
    l.span
        .in_scope(|| l.rtc.sdp_api().accept_answer(pending, answer))?;
    while !(l.is_connected() && r.is_connected()) {
        progress(&mut l, &mut r)?;
    }

    // R (lite) restarts with credentials it supplies, keeping its candidates.
    let second = creds("Y7uI2oP5aS8dF1gH", "j4K7lZ0xC3vB6nM9qW2eR5tY8uI1oP4a");
    let (offer, pending) = r.span.in_scope(|| {
        let mut change = r.rtc.sdp_api();
        let got = change.ice_restart_with(second.clone(), true);
        assert_eq!(got, second);
        change.apply().expect("Should be able to apply changes")
    });
    let offer_sdp = offer.to_sdp_string();
    assert_only_creds(&offer_sdp, &second);
    assert!(
        offer_sdp.contains("a=candidate:"),
        "kept candidates in {offer_sdp}"
    );
    let answer = l.span.in_scope(|| l.rtc.sdp_api().accept_offer(offer))?;
    r.span
        .in_scope(|| r.rtc.sdp_api().accept_answer(pending, answer))?;
    assert_eq!(r._local_ice_creds(), second);
    while !(l.is_connected() && r.is_connected()) {
        if l.duration() > Duration::from_secs(10) {
            panic!("no connectivity after the R restart");
        }
        progress(&mut l, &mut r)?;
    }

    // L restarts; R answers with the credentials it set.
    let third = creds("Q2wE3rT4yU5iO6pA", "s7D8fG9hJ0kL1zX2cV3bN4mQ5wE6rT7y");
    let (offer, pending) = l.span.in_scope(|| {
        let mut change = l.rtc.sdp_api();
        change.ice_restart(true);
        change.apply().expect("Should be able to apply changes")
    });
    let answer = r.span.in_scope(|| {
        let mut api = r.rtc.sdp_api();
        api.set_restart_credentials(third.clone());
        api.accept_offer(offer)
    })?;
    assert_only_creds(&answer.to_sdp_string(), &third);
    assert_eq!(r._local_ice_creds(), third);
    l.span
        .in_scope(|| l.rtc.sdp_api().accept_answer(pending, answer))?;
    while !(l.is_connected() && r.is_connected()) {
        if l.duration() > Duration::from_secs(20) {
            panic!("no connectivity after the L restart");
        }
        progress(&mut l, &mut r)?;
    }

    // A later ordinary renegotiation keeps carrying the current credentials everywhere.
    let (offer, pending) = r.span.in_scope(|| {
        let mut change = r.rtc.sdp_api();
        let _ = change.add_media(MediaKind::Audio, Direction::SendRecv, None, None, None);
        change.apply().expect("an offer")
    });
    assert_only_creds(&offer.to_sdp_string(), &third);
    let answer = l.span.in_scope(|| l.rtc.sdp_api().accept_offer(offer))?;
    r.span
        .in_scope(|| r.rtc.sdp_api().accept_answer(pending, answer))?;

    // Restart credentials are ignored when the offer does not restart ICE.
    let (offer, pending) = l.span.in_scope(|| {
        let mut change = l.rtc.sdp_api();
        let _ = change.add_media(MediaKind::Video, Direction::SendRecv, None, None, None);
        change.apply().expect("an offer")
    });
    let answer = r.span.in_scope(|| {
        let mut api = r.rtc.sdp_api();
        api.set_restart_credentials(creds(
            "Z9xC8vB7nM6aS5dF",
            "g4H3jK2lQ1wE0rT9yU8iO7pA6sD5fG4h",
        ));
        api.accept_offer(offer)
    })?;
    assert_only_creds(&answer.to_sdp_string(), &third);
    l.span
        .in_scope(|| l.rtc.sdp_api().accept_answer(pending, answer))?;

    Ok(())
}
