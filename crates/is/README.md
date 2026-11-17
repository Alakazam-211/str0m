<!-- Changed by Alakazam Labs for sew-str0m-is; the fork's patches are listed in its README. -->
# sew-str0m-is

**sew-str0m-is is a fork of `is`, the ICE crate of [str0m](https://github.com/algesten/str0m).**
Alakazam Labs publishes it for Sew, Alakazam Labs' chat and voice core, together with
[sew-str0m][sew-str0m], its fork of str0m: upstream is 0.12.0 plus the patches listed below.
API documentation: [docs.rs/sew-str0m-is][docs].

**This branch is the fork's working line.** Its manifests build the packages under upstream's names
and versions (`str0m` 0.25.0, `is` 0.12.0); the published packages, sew-str0m 0.25.1 and
sew-str0m-is 0.12.1, are built from the fork's publishing branch, which carries the same code.
The rest of this page describes the published packages.

## Using this fork

```toml
[dependencies]
is = { package = "sew-str0m-is", version = "0.12.1" }
```

Most users get it through sew-str0m (`str0m = { package = "sew-str0m", version = "0.25.1" }`).

- The library names stay upstream's: `is` for this crate and `str0m` for [sew-str0m][sew-str0m],
  so code says `use is::…` and `use str0m::…`.
- It is used instead of upstream `is`. To Cargo it is a different crate: a build that also pulls
  in upstream `is` gets both, and their types do not mix (nor do they with upstream str0m's).
- `str0m-proto` and the `str0m-*` backend crates are upstream's crates from crates.io, and work
  with it.
- The minimum supported Rust version (MSRV) is 1.89 (`rust-version` in Cargo.toml).

## Patches on upstream is 0.12.0

- ice-lite: a controlled agent sends on the highest-priority pair among those nominated in the
  last 3 s, else on the highest-priority nominated pair.
- Remote candidates and STUN messages with port 0 or an unspecified IP (`0.0.0.0`, `::`,
  `::ffff:0.0.0.0`) are ignored ([str0m PR #1148][pr1148], merged upstream).

`CHANGELOG.md`, in this package, lists the changes by version.

## Security

[SECURITY.md][policy] in the sew-crates repository is the policy for this fork: it says
how to report a problem privately.

## Bug reports

Report bugs in our patches at https://github.com/Alakazam-211/sew-crates/issues with the
`crate:sew-str0m` label (`crate:sew-str0m-is` for that crate); bugs in str0m itself go
upstream to [algesten/str0m](https://github.com/algesten/str0m/issues).

---

*The rest of this README is upstream's text for `is`.*

[sew-str0m]: https://crates.io/crates/sew-str0m
[docs]: https://docs.rs/sew-str0m-is
[policy]: https://github.com/Alakazam-211/sew-crates/blob/main/SECURITY.md
[pr1148]: https://github.com/algesten/str0m/pull/1148

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
reused outside the full WebRTC stack. [sew-str0m][sew-str0m] (this fork's
str0m) depends on this crate with default features disabled (it supplies its own HMAC provider through its
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
