# Unreleased

# 0.12.3

sew-str0m-is 0.12.3: sew-str0m-is 0.12.2 with no change to what the code does. It ships with
sew-str0m 0.25.5.

  * The crate forbids unsafe code (`#![forbid(unsafe_code)]`); it had none
  * The package lists its files (`include`); they are the same files as in 0.12.2
  * README: it names sew-str0m 0.25.5, links SECURITY.md where the fork keeps it, and says
    where to report problems, as the fork has no public issue tracker

# 0.12.2

sew-str0m-is 0.12.2: sew-str0m-is 0.12.1 plus this change, merged upstream. It ships with
sew-str0m 0.25.3.

  * Ignore remote candidates and STUN messages with port 0 or an unspecified IP (`0.0.0.0`,
    `::`, `::ffff:0.0.0.0`) (str0m PR #1148)

# 0.12.1

sew-str0m-is 0.12.1: upstream is 0.12.0 (from str0m 0.25.0) plus these patches. It ships with
sew-str0m 0.25.1.

  * ice-lite: a controlled agent sends on the highest-priority pair among those nominated in
    the last 3 s, else on the highest-priority nominated pair
  * Published as `sew-str0m-is`; the library name stays `is`

# 0.12.0

Upstream is 0.12.0, released with str0m 0.25.0. See upstream str0m's CHANGELOG.md.
