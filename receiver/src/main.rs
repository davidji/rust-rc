#![deny(unsafe_code)]
#![deny(warnings)]
#![no_main]
#![no_std]

extern crate panic_semihosting;
extern crate nb;

use core::{
	option::Option,
    convert::Infallible,
};

use embedded_nrf24l01::{
    NRF24L01, RxMode, Payload
};

use stm32f1xx_hal::{
	self,
    prelude::*,
    pac,
    gpio::{ Alternate,  Edge, ExtiPin, Floating, Input, Output, PullUp, PushPull },
    gpio::{ 
        gpioa::{ 
            PA5, PA6, PA7, },
        gpiob::{ 
            PB0, // CE
            PB1, // CSN
            PB10 // IRQ: Note: if you change this pin you must change the EXTI interrupt below
        },
    },
    spi::{ Mode, Phase, Polarity, Spi, Spi1NoRemap },
    stm32::{ SPI1 },
};

use protocol::{ Transmitter, TransmitterMessage::* };
use postcard;

type RadioCe = PB0<Output<PushPull>>;
type RadioCsn = PB1<Output<PushPull>>;
type RadioIrq = PB10<Input<PullUp>>;


type RadioSpi = Spi<SPI1, Spi1NoRemap, 
    (PA5<Alternate<PushPull>>, 
     PA6<Input<Floating>>, 
     PA7<Alternate<PushPull>>)>;

type Radio = NRF24L01<Infallible, RadioCe, RadioCsn, RadioSpi>;

#[rtfm::app(device = stm32f1::stm32f103)]
const APP: () = {

    struct Resources {
        radio: Option<RxMode<Radio>>,
        irq: RadioIrq,
        values: [protocol::Value; 4],
    }

    #[init]
    fn init(_: init::Context) -> init::LateResources {
        // Get access to the device specific peripherals from the peripheral access crate
        let peripherals = pac::Peripherals::take().unwrap();
        // Take ownership over the raw flash and rcc devices and convert them into the corresponding
        // HAL structs
        let mut flash = peripherals.FLASH.constrain();
        let mut rcc = peripherals.RCC.constrain();

        // Freeze the configuration of all the clocks in the system and store the frozen frequencies in
        // `clocks`
        let clocks = rcc.cfgr.use_hse(8.mhz()).freeze(&mut flash.acr);

        // Prepare the alternate function I/O registers
        let mut afio = peripherals.AFIO.constrain(&mut rcc.apb2);

        // Prepare the GPIO peripherals
        let mut gpioa = peripherals.GPIOA.split(&mut rcc.apb2);
        let mut gpiob = peripherals.GPIOB.split(&mut rcc.apb2);
        let ce = gpiob.pb0.into_push_pull_output(&mut gpiob.crl);
        let csn = gpiob.pb1.into_push_pull_output(&mut gpiob.crl);

        let spi_pins = (
            gpioa.pa5.into_alternate_push_pull(&mut gpioa.crl),
            gpioa.pa6.into_floating_input(&mut gpioa.crl),
            gpioa.pa7.into_alternate_push_pull(&mut gpioa.crl),
        );

        let spi_mode = Mode {
            polarity: Polarity::IdleLow,
            phase: Phase::CaptureOnFirstTransition
        };
        
        let spi = Spi::spi1(
            peripherals.SPI1,
            spi_pins,
            &mut afio.mapr,
            spi_mode,
            1.mhz(),
            clocks,
            &mut rcc.apb2
        );

        let radio = NRF24L01::new(ce, csn, spi).unwrap();
        let rx = radio.rx().unwrap();
        
        let mut irq = gpiob.pb10.into_pull_up_input(&mut gpiob.crh);
        irq.make_interrupt_source(&mut afio);
        irq.trigger_on_edge(&peripherals.EXTI, Edge::FALLING);
        irq.enable_interrupt(&peripherals.EXTI);

        init::LateResources {
            radio: Some(rx),
            irq: irq,
            values: [0; 4],
 		}
    }

    #[task(binds = EXTI15_10, priority = 1, 
        resources = [ irq ],
        spawn = [ receive ])]
    fn interrupt(c: interrupt::Context) {
        if c.resources.irq.check_interrupt() {
            c.spawn.receive().unwrap();
            c.resources.irq.clear_interrupt_pending_bit();
        }
    }

    #[task(resources = [radio], spawn=[process])]
    fn receive(c: receive::Context) {
        let mut rx = c.resources.radio.take().unwrap();
        while match rx.can_read() {
            Ok(Some(_)) => {
                match c.spawn.process(rx.read().unwrap()) {
                    Ok(_) => true,
                    Err(_) => false
                }
            },

            Ok(None) => false,
            Err(_) => false,
        } {}
    }

    #[task(resources=[values])]
    fn process(c: process::Context, payload: Payload) {
        let result : postcard::Result<Transmitter> = postcard::from_bytes(&payload);
        match result {
            Ok(Transmitter { correlation_id: _, body }) => {
                match body {
                    ChannelValues(values) => {
                        *c.resources.values = values;
                    }
                }
            },

            Err(_) => {}

        }
    }

    extern "C" {
        fn USART2();
    }
};
