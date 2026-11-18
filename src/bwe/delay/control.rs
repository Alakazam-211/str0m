use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::super::macros::{log_bitrate_estimate, log_delay_variation};
use super::super::{AckedPacket, BandwidthUsage};
use super::arrival_group::ArrivalGroupAccumulator;
use super::rate_control::RateControl;
use super::trendline::TrendlineEstimator;
use crate::rtp_::Bitrate;
use crate::util::{MovingAverage, already_happened};

const MAX_RTT_HISTORY_WINDOW: usize = 32;
const UPDATE_INTERVAL: Duration = Duration::from_millis(25);
/// The maximum time we keep updating our estimate without receiving a TWCC report.
const MAX_TWCC_GAP: Duration = Duration::from_millis(500);
/// RFC 6298: Exponentially Weighted Moving Average smoothing factor for RTT (alpha = 1/8)
const RTT_SMOOTHING_FACTOR: f64 = 0.125;
/// `link_limited`: the fewest packets, the shortest send span, and how much longer the receive
/// span must be.
const LINK_LIMITED_MIN_PACKETS: usize = 4;
const LINK_LIMITED_MIN_SPAN: Duration = Duration::from_millis(10);
const LINK_LIMITED_SPREAD: f64 = 1.1;

/// Delay controller for googcc inspired BWE.
///
/// This controller attempts to estimate the available send bandwidth by looking at the variations
/// in packet arrival times for groups of packets sent together. Broadly, if the delay variation is
/// increasing this indicates overuse.
pub struct DelayController {
    arrival_group_accumulator: ArrivalGroupAccumulator,
    trendline_estimator: TrendlineEstimator,
    rate_control: RateControl,
    /// Last estimate produced, unlike [`next_estimate`] this will always have a value after the
    /// first estimate.
    last_estimate: Option<Bitrate>,
    /// Smoothed RTT using EWMA (RFC 6298, alpha = 1/8).
    smoothed_rtt: MovingAverage,
    /// History of the max RTT derived for each TWCC report (kept for fallback).
    max_rtt_history: VecDeque<Duration>,

    /// The next time we should poll.
    next_timeout: Instant,
    /// The last time we ingested a TWCC report.
    last_twcc_report: Instant,
}

/// Whether `packets` took noticeably longer to arrive than to be sent (the path, not the sender,
/// limits their rate). Needs a few packets received over at least [`LINK_LIMITED_MIN_SPAN`]
/// (above the path's jitter); their send span may be shorter, down to a burst sent at once: a
/// paced frame leaves in a burst of microseconds, and a link that spreads it over tens of
/// milliseconds is the clearest sign of a link-limited rate. Requiring the send span to be
/// [`LINK_LIMITED_MIN_SPAN`] as well rejected exactly those feedbacks, and held a delay-based
/// decrease back for about 1.5 s on a saturated link in our tests (RTT 166 ms rising to 819 ms).
fn link_limited<'a>(packets: impl Iterator<Item = &'a AckedPacket>) -> bool {
    let mut n = 0;
    let (mut send_min, mut send_max) = (None::<Instant>, None::<Instant>);
    let (mut recv_min, mut recv_max) = (None::<Instant>, None::<Instant>);
    for p in packets {
        n += 1;
        send_min = Some(send_min.map_or(p.local_send_time, |t| t.min(p.local_send_time)));
        send_max = Some(send_max.map_or(p.local_send_time, |t| t.max(p.local_send_time)));
        recv_min = Some(recv_min.map_or(p.remote_recv_time, |t| t.min(p.remote_recv_time)));
        recv_max = Some(recv_max.map_or(p.remote_recv_time, |t| t.max(p.remote_recv_time)));
    }
    let (Some(s0), Some(s1), Some(r0), Some(r1)) = (send_min, send_max, recv_min, recv_max) else {
        return false;
    };
    let send_span = s1 - s0;
    let recv_span = r1 - r0;
    n >= LINK_LIMITED_MIN_PACKETS
        && recv_span >= LINK_LIMITED_MIN_SPAN
        && recv_span.as_secs_f64() > send_span.as_secs_f64() * LINK_LIMITED_SPREAD
}

/// The rate at which the path delivered `packets` (bytes after the first to arrive over the
/// receive span), with the same minimums as [`link_limited`].
fn delivered_rate<'a>(packets: impl Iterator<Item = &'a AckedPacket>) -> Option<Bitrate> {
    let mut v: Vec<&AckedPacket> = packets.collect();
    if v.len() < LINK_LIMITED_MIN_PACKETS {
        return None;
    }
    v.sort_by_key(|p| p.remote_recv_time);
    let span = v[v.len() - 1].remote_recv_time - v[0].remote_recv_time;
    if span < LINK_LIMITED_MIN_SPAN {
        return None;
    }
    let mut bytes = crate::rtp_::DataSize::ZERO;
    for p in &v[1..] {
        bytes += p.size;
    }
    Some(bytes / span)
}

impl DelayController {
    pub fn new(initial_bitrate: Bitrate) -> Self {
        Self {
            arrival_group_accumulator: ArrivalGroupAccumulator::default(),
            trendline_estimator: TrendlineEstimator::new(20),
            rate_control: RateControl::new(initial_bitrate, Bitrate::kbps(40), Bitrate::gbps(10)),
            last_estimate: Some(initial_bitrate),
            smoothed_rtt: MovingAverage::new(RTT_SMOOTHING_FACTOR),
            max_rtt_history: VecDeque::default(),
            next_timeout: already_happened(),
            last_twcc_report: already_happened(),
        }
    }

    /// Record a packet from a TWCC report.
    pub fn update(
        &mut self,
        acked: &[AckedPacket],
        acked_bitrate: Option<Bitrate>,
        probe_bitrate: Option<Bitrate>,
        now: Instant,
    ) -> Option<Bitrate> {
        let mut max_rtt = None;

        for acked_packet in acked {
            max_rtt = max_rtt.max(Some(acked_packet.rtt()));
            if let Some(delay_variation) = self
                .arrival_group_accumulator
                .accumulate_packet(acked_packet)
            {
                log_delay_variation!(delay_variation.arrival_delta);

                // Got a new delay variation, add it to the trendline.
                //
                // IMPORTANT: Match WebRTC's TrendlineEstimator time base.
                // WebRTC calls Detect/UpdateThreshold with `arrival_time_ms` (remote receive time),
                // not the local "time we processed this feedback". Using the remote receive time
                // avoids threshold adaptation artifacts when many deltas are processed in one
                // feedback batch (e.g. TWCC reports).
                //
                // Note: We use remote timestamps for relative timing only (computing time deltas
                // between packets). Clock skew doesn't matter since we're measuring trends in
                // delay variations, not absolute times.
                self.trendline_estimator
                    .add_delay_observation(delay_variation, delay_variation.last_remote_recv_time);
            }
        }

        if let Some(rtt) = max_rtt {
            self.update_rtt(rtt);
        }

        let new_hypothesis = self.trendline_estimator.hypothesis();

        // Does the path deliver this feedback's packets (after an overuse episode's last
        // decrease: those sent after it) slower than they were sent?
        let after = self.rate_control.episode_decreased_at();
        let pick = |p: &&AckedPacket| after.is_none_or(|at| p.local_send_time > at);
        let fresh = link_limited(acked.iter().filter(pick));
        self.rate_control.set_fresh_link_limited(Some(fresh));
        let delivered = if fresh {
            delivered_rate(acked.iter().filter(pick))
        } else {
            None
        };
        self.rate_control.set_fresh_delivered(delivered);

        self.update_estimate(
            new_hypothesis,
            acked_bitrate,
            probe_bitrate,
            self.get_smoothed_rtt(),
            now,
        );
        self.last_twcc_report = now;

        self.last_estimate
    }

    pub fn poll_timeout(&self) -> Instant {
        self.next_timeout
    }

    pub fn handle_timeout(&mut self, acked_bitrate: Option<Bitrate>, now: Instant) {
        if !self.trendline_hypothesis_valid(now) {
            // We haven't received a TWCC report in a while. The trendline hypothesis can
            // no longer be considered valid. We need another TWCC report before we can update
            // estimates.
            let next_timeout_in = self
                .get_smoothed_rtt()
                .unwrap_or(MAX_TWCC_GAP)
                .min(UPDATE_INTERVAL);

            // Set this even if we didn't update, otherwise we get stuck in a poll -> handle loop
            // that starves the run loop.
            self.next_timeout = now + next_timeout_in;
            return;
        }

        self.update_estimate(
            self.trendline_estimator.hypothesis(),
            acked_bitrate,
            None,
            self.get_smoothed_rtt(),
            now,
        );
    }

    /// Get the latest estimate.
    pub fn last_estimate(&self) -> Option<Bitrate> {
        self.last_estimate
    }

    /// Whether the delay-based detector currently signals overuse.
    ///
    /// This is useful for gating behaviors (like probing) that would otherwise
    /// re-excite the system while we're already congested.
    pub fn is_overusing(&self) -> bool {
        self.trendline_estimator.hypothesis() == BandwidthUsage::Overuse
    }

    /// Update smoothed RTT using EWMA (RFC 6298, alpha = 1/8).
    fn update_rtt(&mut self, rtt: Duration) {
        // Keep history as fallback in case smoothed RTT is not yet available
        while self.max_rtt_history.len() >= MAX_RTT_HISTORY_WINDOW {
            self.max_rtt_history.pop_front();
        }
        self.max_rtt_history.push_back(rtt);

        // Update smoothed RTT using EWMA: smoothed = (7/8) * smoothed + (1/8) * sample
        self.smoothed_rtt.update(rtt.as_secs_f64());
    }

    /// Get the current smoothed RTT, with fallback to mean of history if not yet available.
    fn get_smoothed_rtt(&self) -> Option<Duration> {
        // Try smoothed RTT first (EWMA)
        if let Some(avg_secs) = self.smoothed_rtt.get() {
            return Some(Duration::from_secs_f64(avg_secs));
        }

        // Fallback to mean of history during initialization
        if self.max_rtt_history.is_empty() {
            return None;
        }

        let sum = self
            .max_rtt_history
            .iter()
            .fold(Duration::ZERO, |acc, rtt| acc + *rtt);
        Some(sum / self.max_rtt_history.len() as u32)
    }

    fn update_estimate(
        &mut self,
        hypothesis: BandwidthUsage,
        observed_bitrate: Option<Bitrate>,
        probe_bitrate: Option<Bitrate>,
        mean_max_rtt: Option<Duration>,
        now: Instant,
    ) {
        // WebRTC's logic from delay_based_bwe.cc MaybeUpdateEstimate():
        // - If we have a probe result, apply it directly and skip delay-based updates
        // - Otherwise, apply normal delay-based rate control
        //
        // This prevents probe results from being immediately overridden by delay-based
        // decreases caused by the probe itself (probes cause temporary queuing delay).

        if let Some(probe_rate) = probe_bitrate {
            // Apply probe result directly, bypassing delay-based updates
            self.rate_control.set_probe_result(probe_rate, now);
            let estimated_rate = self.rate_control.estimated_bitrate();
            log_bitrate_estimate!(estimated_rate.as_f64());
            self.last_estimate = Some(estimated_rate);
        } else if let Some(observed_bitrate) = observed_bitrate {
            // No probe result, apply normal delay-based rate control
            self.rate_control
                .update(hypothesis.into(), observed_bitrate, mean_max_rtt, now);
            let estimated_rate = self.rate_control.estimated_bitrate();

            log_bitrate_estimate!(estimated_rate.as_f64());
            self.last_estimate = Some(estimated_rate);
        }

        // Set this even if we didn't update, otherwise we get stuck in a poll -> handle loop
        // that starves the run loop.
        self.next_timeout = now + UPDATE_INTERVAL;
    }

    /// Whether the current trendline hypothesis is valid i.e. not too old.
    fn trendline_hypothesis_valid(&self, now: Instant) -> bool {
        now.duration_since(self.last_twcc_report)
            <= self
                .get_smoothed_rtt()
                .map(|rtt| rtt * 2)
                .unwrap_or(MAX_TWCC_GAP)
                .min(UPDATE_INTERVAL * 2)
    }
}

#[cfg(test)]
mod test {
    use std::time::{Duration, Instant};

    use super::{AckedPacket, delivered_rate, link_limited};
    use crate::rtp_::DataSize;

    /// Packets of 1000 bytes, the i-th sent at `send[i]` and received at `recv[i]` (offsets in
    /// microseconds from a common start).
    fn packets(send: &[u64], recv: &[u64]) -> Vec<AckedPacket> {
        let t0 = Instant::now();
        let us = |n: u64| t0 + Duration::from_micros(n);
        send.iter()
            .zip(recv)
            .enumerate()
            .map(|(i, (s, r))| AckedPacket {
                seq_no: (i as u64).into(),
                size: DataSize::bytes(1000),
                local_send_time: us(*s),
                remote_recv_time: us(*r),
                local_recv_time: us(*r + 50_000),
            })
            .collect()
    }

    #[test]
    fn a_burst_the_link_spreads_out_is_link_limited() {
        // Sent within 3 microseconds, received over 54 ms.
        let p = packets(&[0, 1, 2, 3], &[0, 18_000, 36_000, 54_000]);
        assert!(link_limited(p.iter()));
        // 3000 bytes after the first over 54 ms.
        let rate = delivered_rate(p.iter()).expect("a delivered rate");
        let bps = rate.as_u64();
        assert!((444_000..=445_000).contains(&bps), "delivered {bps} bit/s");
    }

    #[test]
    fn a_receive_span_within_jitter_is_not_link_limited() {
        let p = packets(&[0, 1, 2, 3], &[0, 2_000, 4_000, 6_000]);
        assert!(!link_limited(p.iter()));
        assert!(delivered_rate(p.iter()).is_none());
    }

    #[test]
    fn packets_arriving_as_spread_out_as_sent_are_not_link_limited() {
        let p = packets(&[0, 10_000, 20_000, 30_000], &[0, 10_000, 20_500, 31_000]);
        assert!(!link_limited(p.iter()));
    }

    #[test]
    fn too_few_packets_are_not_link_limited() {
        let p = packets(&[0, 1, 2], &[0, 18_000, 36_000]);
        assert!(!link_limited(p.iter()));
        assert!(delivered_rate(p.iter()).is_none());
    }
}
