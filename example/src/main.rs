#![no_std]
#![no_main]

use cortex_m::peripheral::NVIC;
use cortex_m_rt::entry;
use cortex_m_semihosting::{debug, hprintln};
use funnel::{flog, funnel, Drain, Logger};
use lm3s6965::{interrupt, Interrupt};
use panic_halt as _;
use ufmt::uwrite;

funnel!(NVIC_PRIO_BITS = 3, {
    1: 32,
    2: 64,
});

#[entry]
fn main() -> ! {
    if let Some(p) = cortex_m::Peripherals::take() {
        unsafe {
            let mut nvic = p.NVIC;
            nvic.set_priority(Interrupt::GPIOA, 224); // prio = 1
            nvic.set_priority(Interrupt::GPIOB, 192); // prio = 2
            NVIC::unmask(Interrupt::GPIOA);
            NVIC::unmask(Interrupt::GPIOB);
        }
    }

    NVIC::pend(Interrupt::GPIOA);
    NVIC::pend(Interrupt::GPIOB);

    let drains = Drain::get_all();

    loop {
        for (i, drain) in drains.iter().cloned().enumerate() {
            for byte in drain {
                hprintln!("{} -> {:?}", i, byte as char).ok();
            }
        }

        debug::exit(debug::EXIT_SUCCESS);
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
