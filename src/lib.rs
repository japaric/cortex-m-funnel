//! A lock-free, wait-free, block-free logger for the ARM Cortex-M architecture
//!
//! (lock-free as in logging doesn't block interrupt handlers; wait-free as in there's no spinning
//! (e.g. CAS loop) to get a handle; and block-free as in the logger never waits for an I/O transfer
//! (e.g. ITM, UART, etc.) to complete)
//!
//! Status: ☢️ **Experimental** ☢️
//!
//! **SUPER IMPORTANT** Using this crate in a threaded environment will result in an unsound
//! program! You have been warned! Also, multi-core support has not been thought out at all so this
//! is likely wrong when used in multi-core context.
//!
//! # Working principle
//!
//! There's one ring buffer per priority level. Logging from an interrupt / exception handler will
//! simply write the message into one of these ring buffers. Thus logging is effectively 'I/O
//! less' and as fast as a `memcpy`. Only the 'thread handler' (AKA `main` or `idle` in RTFM apps)
//! can drain these ring buffers into an appropriate I/O sink (e.g. the ITM).
//!
//! Nothing is without trade-offs in this life; this logger uses plenty of static memory (i.e.
//! RAM) in exchange for fast and predictable logging performance. Also, compared to loggers that
//! directly do I/O this logger will, in overall, spend more CPU cycles to log the same amount of
//! data but most of the work will be done at the lowest priority making logging in interrupt
//! handlers much faster.
//!
//! # Examples
//!
//! ## Usual setup
//!
//! Application crate:
//!
//! ``` ignore
//! // aligned = "0.3.2"
//! use aligned::Aligned;
//! use cortex_m::itm;
//!
//! use funnel::{Drain, flog, funnel};
//!
//! // `NVIC_PRIO_BITS` is the number of priority bits supported by the device
//! //
//! // The `NVIC_PRIO_BITS` value can be a literal integer (e.g. `3`) or a path to a constant
//! // (`stm32f103xx::NVIC_PRIO_BITS`)
//! //
//! // This macro call can only appear *once* in the dependency graph and *must* appear if
//! // the `flog!` macro or the `Logger::get()` API is used anywhere in the dependency graph
//! funnel!(NVIC_PRIO_BITS = 3, {
//!      // syntax: $logical_priority : $ring_buffer_size_in_bytes
//!      // to get better performance use sizes that are a power of 2
//!      1: 32,
//!      2: 64,
//!
//!      // not listing a priority here disables logging at that priority level
//!      // entering the wrong NVIC_PRIO_BITS value will disable most loggers
//! });
//!
//! #[entry]
//! fn main() -> ! {
//!     // ..
//!     let mut itm: ITM = /* .. */;
//!
//!     let drains = Drain::get_all();
//!
//!     let mut buf = Aligned([0; 32]); // 4-byte aligned buffer
//!     loop {
//!         for (i, drain) in drains.iter().enumerate() {
//!             'l: loop {
//!                 let n = drain.read(&mut buf).len();
//!
//!                 // this drain is empty
//!                 if n == 0 {
//!                     break 'l;
//!                 }
//!
//!                 // we need this coercion or the slicing below won't do the right thing
//!                 let buf: &Aligned<_, [_]> = &buf;
//!
//!                 // will send data in 32-bit chunks
//!                 itm::write_aligned(&mut itm.stim[i], &buf[..n]);
//!             }
//!         }
//!     }
//! }
//!
//! // logical_priority = 1 (nvic_priority = 224)
//! #[interrupt]
//! fn GPIOA() {
//!     flog!("GPIOA");
//!     foo(0);
//!     // ..
//! }
//!
//! // logical_priority = 2 (nvic_priority = 192)
//! #[interrupt]
//! fn GPIOB() {
//!     flog!("GPIOB");
//!     foo(1);
//!     // ..
//! }
//!
//! fn foo(x: i32) {
//!     // this macro can appear in libraries
//!     flog!("foo({})", x);
//!     // ..
//! }
//! ```
//!
//! ## `Logger`
//!
//! The overhead of each `flog!` call can be reduced using one of the `uwrite!` macros on a
//! `Logger`. A `Logger` can only be obtained using the `Logger::get()` constructor.
//!
//! ``` ignore
//! use funnel::Logger;
//!
//! #[interrupt]
//! fn GPIOC() {
//!     if let Some(mut logger) = Logger::get() {
//!          uwriteln!(logger, "{}", 100).ok();
//!          uwriteln!(logger, "{:?}", some_value).ok();
//!     }
//! }
//! ```
//!
//! # Benchmarks
//!
//! Ran on Cortex-M3 core clocked at 8 MHz and configured with 0 Flash wait cycles.
//!
//! | Code                         | Cycles  |
//! |------------------------------|---------|
//! | `flog!("")`                  | 36      |
//! | `uwriteln!(logger, "")`      | 15      |
//! | `drain("")`                  | 27      |
//! | `flog!("{}", S)`             | 331-369 |
//! | `uwriteln!(logger, "{}", S)` | 308-346 |
//! | `drain(S)`                   | 863-916 |
//! | `iprintln!(_, "{}", S)`      | 1652    |
//! | `flog!("{}", N)`             | 348-383 |
//! | `uwriteln!(logger, "{}", N)` | 329-364 |
//! | `drain(N)`                   | 217-230 |
//!
//! Where `S` is a 45-byte long string, `N = usize::max_value()`, the `drain` function is
//! `ptr::read_volatile`-ing each byte and the ITM was clocked at 2 MHz.
//!
//! # Potential improvements / alternatives
//!
//! Instead of draining the ring buffers at the lowest priority one could drain the buffers using
//! the debugger using something like [SEGGER's Real Time Transfer][rtt] mechanism. The
//! implementation would need to change to properly support this form of parallel draining.
//!
//! [rtt]: https://www.segger.com/products/debug-probes/j-link/technology/about-real-time-transfer/

#![deny(missing_docs)]
#![deny(warnings)]
#![no_std]

use core::{
    cell::UnsafeCell,
    cmp, ptr,
    sync::atomic::{self, AtomicUsize, Ordering},
};

use ufmt::uWrite;

/// Declares loggers for each priority level
pub use cortex_m_funnel_macros::funnel;
#[doc(hidden)]
pub use ufmt::uwriteln;

/// IMPLEMENTATION DETAIL
// `static [mut]` variables cannot contain references to `static mut` variables so we lie about the
// `Sync`-ness of `Inner` to be able to put references to it in `static` variables. Only the
// `funnel!` macro uses this type -- end users will never see this type.
#[doc(hidden)]
#[repr(C)]
pub struct Inner<B>
where
    B: ?Sized,
{
    write: UnsafeCell<usize>,
    read: UnsafeCell<usize>,
    buffer: UnsafeCell<B>,
}

unsafe impl<B> Sync for Inner<B> where B: ?Sized {}

impl<B> Inner<B> {
    // IMPLEMENTATION DETAIL
    #[doc(hidden)]
    pub const fn new(buffer: B) -> Self {
        Self {
            write: UnsafeCell::new(0),
            read: UnsafeCell::new(0),
            buffer: UnsafeCell::new(buffer),
        }
    }
}

/// A logger tied a particular priority level
// NOTE: NOT `Sync` or `Send`
#[repr(transparent)]
pub struct Logger {
    inner: &'static Inner<[u8]>,
}

impl Logger {
    /// Gets the `funnel` logger associated to the caller's priority level
    ///
    /// This returns `None` if no logger was associated to the priority level
    pub fn get() -> Option<Self> {
        if cfg!(not(cortex_m)) {
            return None;
        }

        // Cortex-M MMIO registers
        const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
        const NVIC_IPR: *const u32 = 0xE000_E400 as *const u32;

        extern "Rust" {
            // NOTE The expansion of `funnel!` declares `__funnel_drains` as a function with signature
            // `fn() -> Option<&'static Inner<[u8]>>` so here we are implicitly transmuting `&'static
            // Inner<[u8]>` into `Logger` but this should be fine because they are equivalent due to
            // `#[repr(transparent)]`
            fn __funnel_logger(nvic_prio: u8) -> Option<Logger>;
        }

        unsafe {
            let icsr = SCB_ICSR.read_volatile() as u8;

            if icsr == 0 {
                // thread mode
                None
            } else if icsr < 16 {
                // TODO do something about exceptions -- NMI and HardFault are annoying because they
                // have exceptional priorities
                None
            } else {
                // assuming ARMv6-M (the lowest common denominator), IPR is *not* byte addressable
                // so we perform word-size reads
                let nr = icsr - 16;

                // NOTE `nr` will always be less than `256`
                let ipr = NVIC_IPR.add((nr >> 2) as usize).read_volatile();

                let nvic_prio = (ipr >> (8 * (nr % 4))) as u8;

                __funnel_logger(nvic_prio)
            }
        }
    }

    // This function is *non*-reentrant but `Logger` is `!Sync` so each `Logger`s is constrained to
    // a single priority level (therefore no preemption / overlap can occur on any single `Logger`
    // instance)
    fn log(&self, s: &str) -> Result<(), ()> {
        unsafe {
            // NOTE we use `UnsafeCell` instead of `AtomicUsize` because we want the unique
            // reference (`&mut-`) semantics; this logger has exclusive access to the `write`
            // pointer
            let write = &mut *self.inner.write.get();
            let buffer = &mut *self.inner.buffer.get();

            let input = s.as_bytes();

            let blen = buffer.len();
            let ilen = input.len();

            if ilen > blen {
                // early exit to hint the optimizer that `blen` can't be `0`
                return Err(());
            }

            // NOTE we use `UnsafeCell` instead of `AtomicUsize` because we want this operation to
            // return the same value when calling `log` consecutively
            let read = *self.inner.read.get();

            if blen >= ilen + (*write).wrapping_sub(read) {
                // FIXME (?) this is *not* always optimized to a right shift (`lsr`) when `blen` is
                // a power of 2 -- instead we get an `udiv` which is slower (?).
                let w = *write % blen;

                // NOTE we use `ptr::copy_nonoverlapping` instead of `copy_from_slice` to avoid
                // panicking branches
                if w + ilen > blen {
                    // two memcpy-s
                    let mid = blen - w;
                    // buffer[w..].copy_from_slice(&input[..mid]);
                    ptr::copy_nonoverlapping(input.as_ptr(), buffer.as_mut_ptr().add(w), mid);
                    // buffer[..ilen - mid].copy_from_slice(&input[mid..]);
                    ptr::copy_nonoverlapping(
                        input.as_ptr().add(mid),
                        buffer.as_mut_ptr(),
                        ilen - mid,
                    );
                } else {
                    // single memcpy
                    // buffer[w..w + ilen].copy_from_slice(&input);
                    ptr::copy_nonoverlapping(input.as_ptr(), buffer.as_mut_ptr().add(w), ilen);
                }

                *write = (*write).wrapping_add(ilen);

                Ok(())
            } else {
                Err(())
            }
        }
    }
}

impl uWrite for Logger {
    type Error = ();

    fn write_str(&mut self, s: &str) -> Result<(), ()> {
        self.log(s)
    }
}

/// Logs a string
///
/// Syntax matches `println!`. You need to depend on the `ufmt` crate to use this macro.
///
/// NOTE a newline is always appended to the end
#[macro_export]
macro_rules! flog {
    ($($tt:tt)*) => {{
        if let Some(mut logger) = $crate::Logger::get() {
            $crate::uwriteln!(logger, $($tt)*)
        } else {
            Ok(())
        }
    }};
}

/// A drain retrieves the data written into a `Logger`
// NOTE: NOT `Sync` or `Send`
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Drain {
    inner: &'static Inner<[u8]>,
}

impl Drain {
    /// The drain endpoint of each ring buffer, highest priority first
    pub fn get_all() -> &'static [Self] {
        if cfg!(not(cortex_m)) {
            return &[];
        }

        // NOTE The expansion of `funnel!` declares `__funnel_drains` as a function with signature
        // `fn() -> &'static [&'static Inner<[u8]>]` so here we are implicitly transmuting `&'static
        // Inner<[u8]>` into `Drain` but this should be fine because they are equivalent due to
        // `#[repr(transparent)]`
        extern "Rust" {
            fn __funnel_drains() -> &'static [Drain];
        }

        unsafe { __funnel_drains() }
    }

    /// Copies the contents of the `Logger` ring buffer into the given buffer
    // NOTE this is basically `heapless::spsc::Consumer::dequeue`
    pub fn read<'b>(&self, buf: &'b mut [u8]) -> &'b [u8] {
        unsafe {
            // NOTE we use `UnsafeCell` instead of `AtomicUsize` because we want the unique
            // reference (`&mut-`) semantics; this drain has exclusive access to the `read`
            // pointer for the duration of this function call
            let readf = &mut *self.inner.read.get();
            let writef: *const AtomicUsize = self.inner.write.get() as *const _;
            let blen = (*self.inner.buffer.get()).len();
            let p = (*self.inner.buffer.get()).as_ptr();

            // early exit to hint the compiler that `n` is not `0`
            if blen == 0 {
                return &[];
            }

            let read = *readf;
            // XXX on paper, this is insta-UB because `Logger::log` has a unique reference
            // (`&mut-`) to the `write` field and this operation require a shared reference (`&-`)
            // to the same field. At runtime, this load is atomic (happens in a single instruction)
            // so any modification done by an interrupt handler (via `Logger::log`) can *not* result
            // in a data race (e.g. torn read or write). To properly avoid any theoretical UB we
            // would need to something like `atomic_load(a_raw_pointer_to_write)`, which exist but
            // it's unstable (`intrinsics::atomic_load`), *plus* `&raw write` (RFC #2582), which has
            // not been implemented. In practice, as long as this produces a fresh value each time
            // is called (instead of cached on the stack) we should be fine.
            let write = (*writef).load(Ordering::Relaxed);
            atomic::compiler_fence(Ordering::Acquire); // ▼

            if write > read {
                // number of bytes to copy
                let c = cmp::min(buf.len(), write.wrapping_sub(read));
                // FIXME (?) this is *not* always optimized to a right shift (`lsr`) when `n` is
                // a power of 2 -- instead we get an `udiv` which is slower.
                let r = read % blen;

                // NOTE we use `ptr::copy_nonoverlapping` instead of `copy_from_slice` to avoid
                // panicking branches
                if r + c > blen {
                    // two memcpy-s
                    let mid = blen - r;
                    // buf[..mid].copy_from_slice(&buffer[r..]);
                    ptr::copy_nonoverlapping(p.add(r), buf.as_mut_ptr(), mid);
                    // buf[mid..mid + c].copy_from_slice(&buffer[..c - mid]);
                    ptr::copy_nonoverlapping(p, buf.as_mut_ptr().add(mid), c - mid);
                } else {
                    // single memcpy
                    // buf[..c].copy_from_slice(&buffer[r..r + c]);
                    ptr::copy_nonoverlapping(p.add(r), buf.as_mut_ptr(), c);
                }

                atomic::compiler_fence(Ordering::Release); // ▲
                *readf = (*readf).wrapping_add(c);

                // &buf[..c]
                buf.get_unchecked(..c)
            } else {
                &[]
            }
        }
    }
}

impl Iterator for Drain {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        self.read(&mut [0]).first().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::{Drain, Inner, Logger};

    #[test]
    fn sanity() {
        static INNER: Inner<[u8; 32]> = Inner::new([0; 32]);

        let inner = &INNER;
        let m = "Hello, world!";
        let logger = Logger { inner };
        logger.log(m).unwrap();
        unsafe {
            assert!((*logger.inner.buffer.get()).starts_with(m.as_bytes()));
        }
    }

    #[test]
    fn drain() {
        static INNER: Inner<[u8; 32]> = Inner::new([0; 32]);

        let inner = &INNER;
        let logger = Logger { inner };
        let mut drain = Drain { inner };

        assert_eq!(drain.next(), None);

        logger.log("A").unwrap();
        assert_eq!(drain.next(), Some(b'A'));
        assert_eq!(drain.next(), None);

        logger.log("B").unwrap();
        assert_eq!(drain.next(), Some(b'B'));
        assert_eq!(drain.next(), None);

        logger.log("CD").unwrap();
        assert_eq!(drain.next(), Some(b'C'));
        assert_eq!(drain.next(), Some(b'D'));
        assert_eq!(drain.next(), None);
    }

    #[test]
    fn read() {
        static INNER: Inner<[u8; 16]> = Inner::new([0; 16]);

        let inner = &INNER;
        let logger = Logger { inner };
        let drain = Drain { inner };

        let mut buf = [0; 8];
        logger.log("Hello, world!").unwrap();
        assert_eq!(drain.read(&mut buf), b"Hello, w");
        assert_eq!(drain.read(&mut buf), b"orld!");
        assert_eq!(drain.read(&mut buf), b"");

        // NOTE the ring buffer will wrap around with this operation
        logger.log("Hello, world!").unwrap();
        assert_eq!(drain.read(&mut buf), b"Hello, w");
        assert_eq!(drain.read(&mut buf), b"orld!");
        assert_eq!(drain.read(&mut buf), b"");
    }

    #[test]
    fn split_write() {
        const N: usize = 32;
        const M: usize = 24;
        static INNER: Inner<[u8; N]> = Inner::new([0; N]);

        let m = "Hello, world!";
        let inner = &INNER;
        unsafe {
            // fake read/write pointers
            *inner.read.get() = M;
            *inner.write.get() = M;

            let logger = Logger { inner };
            logger.log(m).unwrap();
            let m = m.as_bytes();
            let buffer = &*logger.inner.buffer.get();
            assert_eq!(buffer[M..], m[..(N - M)]);
            assert_eq!(buffer[..(m.len() - (N - M))], m[(N - M)..]);
        }
    }

    #[test]
    fn wrap_around() {
        static INNER: Inner<[u8; 32]> = Inner::new([0; 32]);

        let m = "Hello, world!";
        let inner = &INNER;
        unsafe {
            // fake read/write pointers
            *inner.read.get() = usize::max_value();
            *inner.write.get() = usize::max_value();

            let logger = Logger { inner };
            logger.log(m).unwrap();

            let buffer = &*logger.inner.buffer.get();
            assert_eq!(buffer.last(), Some(&b'H'));
            assert_eq!(buffer[..m.len() - 1], m.as_bytes()[1..]);
        }
    }
}
