# sew-str0m-is

**sew-str0m-is is a fork of [str0m](https://github.com/algesten/str0m) with Sew's patches.** It is not
affiliated with or endorsed by the str0m authors.

- **Upstream base:** str0m 0.24.0 (this crate: is 0.11.1), commit
  [`95e5038c`](https://github.com/algesten/str0m/commit/95e5038c6f45de6646d8e3f4c2bca6207a5bedde) (upstream tag `0.24`).
- **This crate:** the two ice-lite patches below, and sew-str0m-proto in place of str0m-proto.
- **The fork's patches** (in sew-str0m and sew-str0m-is):
  - Let the application supply its own ICE credentials on restart ([`0926761e`](https://github.com/Alakazam-211/str0m/commit/0926761e50755e1f4b37ce124061f6b439449d79), test [`6b8aeaab`](https://github.com/Alakazam-211/str0m/commit/6b8aeaabac518c9aae64c361beb20ba53e2af52d)).
  - `StreamTx::set_nack_policy`: per-packet NACK limits ([`b9efb669`](https://github.com/Alakazam-211/str0m/commit/b9efb66975e9c70a162621a4097fc048e67e3254)).
  - Resend in-band when the packet's payload type has no RTX payload type ([`6aa71dce`](https://github.com/Alakazam-211/str0m/commit/6aa71dce9d5d135eb6e6ab0cdecf3ece55b77b20)).
  - Do not pad on a stream whose media stopped sending ([`de51e8f9`](https://github.com/Alakazam-211/str0m/commit/de51e8f9144b595e1be1ddf90e3a00892427bd66)).
  - Loop instead of recursing in `Rtc::do_poll_output` ([`73fce91c`](https://github.com/Alakazam-211/str0m/commit/73fce91c6fbe566d6610dcc1e28e8c66e625005c)).
  - ICE (`is`): an ice-lite controlled agent sends on the most recently nominated pair ([`f55548ca`](https://github.com/Alakazam-211/str0m/commit/f55548ca45becb29527f0b62ffbb4bca6054ad51)).
  - ICE (`is`): ice-lite renomination follows a fresh nomination, not the latest one ([`d1a7722f`](https://github.com/Alakazam-211/str0m/commit/d1a7722f66cf3133c6eb7fe0ec6c4659db97f692)).
  - `reset_stream_tx` keeps the send stream under its new SSRC ([`a6378f7e`](https://github.com/Alakazam-211/str0m/commit/a6378f7e5ff65009df6724152b05ee33ff4b0a28)).
  - `MediaEgressStats` carries `packets_resent` and `bytes_resent` ([`f80c8fa0`](https://github.com/Alakazam-211/str0m/commit/f80c8fa09323778b181923a840a9fd36e1c8d951)).
  - Capped initial probes no longer lower the estimate below the start estimate ([`1ec02c2f`](https://github.com/Alakazam-211/str0m/commit/1ec02c2f930fd5f3da8ce460e90d6f66c46abf0e); upstream PR [str0m#1126](https://github.com/algesten/str0m/pull/1126)).
  - A delay-based decrease needs the link, not the sender, to limit the acknowledged packets ([`76b78755`](https://github.com/Alakazam-211/str0m/commit/76b78755339d3dc8aa5c9e440cdee7f97e76e639); upstream issue [str0m#1127](https://github.com/algesten/str0m/issues/1127)).
  - One upstream contributor's change ahead of an upstream release: NACK retransmission without RTX via in-band resend on the main SSRC, by Jury Jaraševič ([`7a925e9c`](https://github.com/Alakazam-211/str0m/commit/7a925e9c0fbfe68725ed177c6478276a6093165a); [str0m#980](https://github.com/algesten/str0m/pull/980)).
- **The fork's crates:** sew-str0m, sew-str0m-proto, sew-str0m-is and sew-str0m-aws-lc-rs, all
  depending on [sew-dimpl](https://crates.io/crates/sew-dimpl) (the dimpl fork). sew-str0m ships
  the aws-lc-rs crypto provider only; upstream's other providers are not part of the fork.
- **Use it** under the library's upstream name, so code still says `is::…`:
  `is = { package = "sew-str0m-is", version = "0.11.2" }`.
- **Licence:** upstream's, MIT OR Apache-2.0 ([LICENSE-MIT.txt](LICENSE-MIT.txt),
  [LICENSE-APACHE.txt](LICENSE-APACHE.txt)). Credit for str0m belongs to its authors.
- **Security fixes** from upstream are merged into this fork promptly.
- **Returning to upstream:** once an upstream release carries these fixes, Sew goes back to
  upstream str0m and this fork gets a final note here.

Release notes: [CHANGELOG.md](https://github.com/Alakazam-211/str0m/blob/sew-crates-io/CHANGELOG.md).
Upstream's README follows.

---

# is

## is -- ICE in Sans-IO style

Standalone [ICE][rfc8445] agent extracted from the [str0m][str0m] WebRTC library.

ICE is the protocol used to establish peer-to-peer connectivity across
the internet. It coordinates STUN connectivity checks over every
possible network path between two peers and picks the best one,
handling NAT traversal along the way.

This crate implements the ICE agent -- the state machine that drives
candidate pairing, connectivity checks, and nomination -- without
performing any I/O itself.

## Sans-IO

This is a [Sans-IO][sansio] implementation. The `IceAgent` never touches
a socket. Instead you drive it in a loop:

1. `poll_timeout()` -- when to wake up next.
2. `handle_timeout(now)` -- advance timers.
3. `poll_transmit()` -- STUN packets to send.
4. `handle_packet(now, pkt)` -- feed incoming STUN.
5. `poll_event()` -- state changes and nominations.

This makes the agent easy to embed in any async runtime, event loop, or
test harness.

## Quick start

```rust
use is::{IceAgent, IceCreds, Candidate};

// Create an agent with random credentials.
let mut agent = IceAgent::new(IceCreds::new());

// Tell the agent about a local socket.
let addr = "192.168.1.100:5000".parse().unwrap();
agent.add_local_candidate(Candidate::host(addr, "udp").unwrap());

// Exchange credentials and candidates with the remote peer via your
// signalling channel, then run the loop above until
// IceConnectionState::Connected.
```

## Address discovery is external

The agent does not enumerate network interfaces, query STUN servers, or
allocate TURN relays. All of that is the caller's responsibility.
Discovered addresses are fed in via `add_local_candidate()` at any time
(trickle ICE).

## Relationship to str0m

This crate was extracted from [str0m][str0m] so the ICE agent can be
reused outside the full WebRTC stack. str0m depends on this crate with
default features disabled (it supplies its own HMAC provider through its
pluggable crypto backend).

When used standalone the default `sha1` feature provides
`DefaultSha1HmacProvider` and a convenient `IceAgent::new()` constructor.
Disable it if you want to supply your own provider via
`IceAgent::with_hmac()`.

## Design decisions

The implementation makes several simplifying assumptions that suit
WebRTC usage:

- **STUN/TURN are external** -- see above.
- **BUNDLE assumed** -- one agent, one data stream, one component (RTP).
- **Trickle ICE only** -- candidates can arrive at any time.
- **No frozen check state** -- single data stream means pairs go straight
  to waiting.
- **Nomination is not final** -- the agent can change its nominated pair
  as network conditions shift, without an ICE restart.
- **Ice-lite supported** -- the agent can act as a controlled ice-lite
  endpoint for server-side use.

For the full rationale and list of assumptions see the
[design document][design].

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| `sha1`  | yes     | Provides `DefaultSha1HmacProvider` and `IceAgent::new()` |
| `pii`   | no      | Redacts IP addresses and other PII from log output |

[rfc8445]: https://datatracker.ietf.org/doc/html/rfc8445
[str0m]: https://crates.io/crates/str0m
[sansio]: https://sans-io.readthedocs.io/
[design]: https://github.com/algesten/str0m/blob/main/docs/ice.md

License: MIT OR Apache-2.0
