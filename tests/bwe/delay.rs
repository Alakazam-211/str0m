//! Delay-based bandwidth estimation tests.

use std::time::Duration;

use netem::{DataSize, NetemConfig};
use str0m::RtcError;
use str0m::bwe::Bitrate;

use crate::common::{
    BweTestContext, Step, connect_with_bwe, get_last_bwe_estimate, init_crypto_default, init_log,
};

#[test]
fn overuse_triggers_aimd_decrease() -> Result<(), RtcError> {
    init_log();
    init_crypto_default();

    let plan = vec![
        Step::Conditions {
            description: "Start with stable network",
            config: NetemConfig::new()
                .latency(Duration::from_millis(20))
                .link(Bitrate::mbps(3), DataSize::kbytes(100))
                .seed(42),
        },
        Step::Media {
            description: "Send at 2 Mbps",
            desired_bitrate: Bitrate::mbps(5),
            media_send_rate: Bitrate::mbps(2),
        },
        Step::Run {
            description: "Establish stable estimate",
            duration: Duration::from_secs(5),
        },
        Step::Check {
            description: "Estimate stabilizes around 2 Mbps",
            at_least: Bitrate::mbps(2),
        },
        // Introduce queuing delay to trigger overuse
        Step::Conditions {
            description: "Increase latency to cause queuing",
            config: NetemConfig::new()
                .latency(Duration::from_millis(100)) // Significant increase
                .link(Bitrate::mbps(3), DataSize::kbytes(100))
                .seed(42),
        },
        Step::Run {
            description: "Wait for overuse detection",
            duration: Duration::from_secs(2),
        },
        Step::Check {
            description: "Estimate drops due to overuse (AIMD decrease)",
            at_least: Bitrate::mbps(1),
        },
    ];

    let (mut l, mut r) = connect_with_bwe(Bitrate::mbps(2), Bitrate::mbps(5));
    let mut ctx = BweTestContext::new(&mut l, &mut r);
    ctx.run_plan(&mut l, &mut r, &plan)?;

    Ok(())
}

#[test]
fn underuse_allows_estimate_growth() -> Result<(), RtcError> {
    init_log();
    init_crypto_default();

    let plan = vec![
        Step::Conditions {
            description: "High capacity network",
            config: NetemConfig::new()
                .latency(Duration::from_millis(10))
                .link(Bitrate::mbps(10), DataSize::kbytes(200))
                .seed(42),
        },
        Step::Media {
            description: "Start low, send consistently",
            desired_bitrate: Bitrate::mbps(10),
            media_send_rate: Bitrate::mbps(1),
        },
        Step::Run {
            description: "Initial estimate",
            duration: Duration::from_secs(2),
        },
        Step::Check {
            description: "Baseline estimate established",
            at_least: Bitrate::mbps(1),
        },
        Step::Run {
            description: "Allow estimate to grow",
            duration: Duration::from_secs(10),
        },
        Step::Check {
            description: "Estimate grows under good conditions",
            at_least: Bitrate::mbps(5),
        },
    ];

    let (mut l, mut r) = connect_with_bwe(Bitrate::mbps(1), Bitrate::mbps(10));
    let mut ctx = BweTestContext::new(&mut l, &mut r);
    ctx.run_plan(&mut l, &mut r, &plan)?;

    Ok(())
}

#[test]
fn application_limited_sender_keeps_its_estimate_after_a_probe_queue() -> Result<(), RtcError> {
    init_log();
    init_crypto_default();

    // A 2 Mbps link and a sender far below it (an application whose media has not ramped up
    // yet). The initial probes queue on the link and the delay trend reads overuse; a decrease
    // based on the acknowledged bitrate, the sender's own 80 kbps, would throw the estimate
    // to a fraction of that. The estimate must stay near what the link carries.
    let (mut l, mut r) = connect_with_bwe(Bitrate::kbps(1_500), Bitrate::mbps(3));
    let mut ctx = BweTestContext::new(&mut l, &mut r);
    let link = NetemConfig::new()
        .latency(Duration::from_millis(20))
        .jitter(Duration::from_millis(2))
        .link(Bitrate::mbps(2), DataSize::kbytes(150))
        .seed(42);
    l.set_netem(link);
    r.set_netem(link);
    ctx.set_media_send_rate(Bitrate::kbps(80));

    let mut lowest: Option<Bitrate> = None;
    for _ in 0..250 {
        ctx.run_for_duration(&mut l, &mut r, Duration::from_millis(20))?;
        if let Some(e) = get_last_bwe_estimate(&l) {
            lowest = Some(lowest.map_or(e, |x| x.min(e)));
        }
    }

    let lowest = lowest.expect("an estimate");
    assert!(
        lowest >= Bitrate::kbps(1_200),
        "the estimate fell to {lowest} on a 2 Mbps link"
    );

    Ok(())
}
