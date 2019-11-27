# `cortex-m-funnel`

> A lock-free, wait-free, block-free logger for the ARM Cortex-M architecture

(lock-free as in logging doesn't block interrupt handlers; wait-free as in
there's no spinning (e.g. CAS loop) to get a handle; and block-free as in the
logger never waits for an I/O transfer (e.g. ITM, UART, etc.) to complete)

Status: ☢️ **Experimental** ☢️ (ALPHA PRE-RELEASE)

## [Documentation](https://docs.rs/cortex-m-funnel)

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
