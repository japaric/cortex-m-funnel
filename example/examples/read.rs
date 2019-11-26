#![no_std]
#![no_main]

use aligned::Aligned;
use cortex_m::{itm, peripheral::NVIC};
use cortex_m_rt::entry;
use funnel::{flog, funnel, Drain, Logger};
use lm3s6965::{interrupt, Interrupt};
use panic_never as _;
use ufmt::uwrite;

funnel!(NVIC_PRIO_BITS = 3, {
    1: 32,
    2: 64,
});

#[entry]
fn main() -> ! {
    let mut itm = if let Some(p) = cortex_m::Peripherals::take() {
        unsafe {
            let mut nvic = p.NVIC;
            nvic.set_priority(Interrupt::GPIOA, 224); // prio = 1
            nvic.set_priority(Interrupt::GPIOB, 192); // prio = 2
            NVIC::unmask(Interrupt::GPIOA);
            NVIC::unmask(Interrupt::GPIOB);
            p.ITM
        }
    } else {
        // unreachable
        loop {}
    };

    NVIC::pend(Interrupt::GPIOA);
    NVIC::pend(Interrupt::GPIOB);

    let drains = Drain::get_all();

    let mut buf = Aligned([0; 32]);
    loop {
        for (i, drain) in drains.iter().enumerate() {
            'l: loop {
                let n = drain.read(&mut *buf).len();
                if n == 0 {
                    // drain is empty
                    break 'l;
                }
                let buf: &Aligned<_, [_]> = &buf;
                if let Some(stim) = itm.stim.get_mut(i) {
                    itm::write_aligned(stim, &buf[..n]);
                }
            }
        }
    }
}

#[interrupt]
fn GPIOA() {
    if let Some(mut logger) = Logger::get() {
        uwrite!(logger, "A").ok();
    }

    flog!("B").ok();
}

#[interrupt]
fn GPIOB() {
    if let Some(mut logger) = Logger::get() {
        uwrite!(logger, "C").ok();
    }

    flog!("D").ok();
}
