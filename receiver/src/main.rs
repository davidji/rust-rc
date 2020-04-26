#![deny(unsafe_code)]
#![deny(warnings)]
#![no_main]
#![no_std]

extern crate panic_semihosting;
extern crate nb;

use cortex_m::{ singleton };

use core::{
	option::Option,
    convert::Infallible,
    fmt::{ self, Write },
};

use embedded_nrf24l01::{
    NRF24L01, RxMode, Payload
};

use stm32f4xx_hal::{
	self,
    prelude::*,
    gpio::{ AF5, Alternate,  Edge, ExtiPin, Input, Output, PullUp, PushPull },
    gpio::{ 
        gpioa::{ 
            PA5, PA6, PA7, },
        gpiob::{ 
            PB0, // CE
            PB1, // CSN
            PB10 // IRQ: Note: if you change this pin you must change the EXTI interrupt below
        },
        gpioc::{
            PC13
        }
    },
    otg_fs::{ USB, UsbBus, UsbBusType },
    spi::{ Mode, Phase, Polarity, Spi },
    stm32::{ SPI1, TIM2},
    timer::{ Timer, Event },
};


use usb_device::{
    bus,
    prelude::*
};

use usbd_serial;

use protocol::{ Transmitter, TransmitterMessage::* };
use postcard;

type RadioCe = PB0<Output<PushPull>>;
type RadioCsn = PB1<Output<PushPull>>;
type RadioIrq = PB10<Input<PullUp>>;


type RadioSpi = Spi<SPI1,
    (PA5<Alternate<AF5>>, 
     PA6<Alternate<AF5>>, 
     PA7<Alternate<AF5>>)>;

type Radio = NRF24L01<Infallible, RadioCe, RadioCsn, RadioSpi>;

pub struct ConsoleSerial(usbd_serial::SerialPort<'static, UsbBusType>);

impl fmt::Write for ConsoleSerial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        match self.0.write(s.as_bytes()) {
            Ok(count) if count == s.len() => Ok(()),
            Ok(_) => Err(fmt::Error {}),
            Err(_) => Err(fmt::Error {}),
        }
    }
}

#[rtfm::app(device = stm32f4::stm32f411, peripherals=true)]
const APP: () = {

    struct Resources {
        radio: Option<RxMode<Radio>>,
        irq: RadioIrq,
        values: [protocol::Value; 4],
        usb_dev: UsbDevice<'static, UsbBusType>,
        usb_serial: ConsoleSerial,
        timer: Timer<TIM2>,
        led: PC13<Output<PushPull>>,
    }

    #[init]
    fn init(c: init::Context) -> init::LateResources {
        static mut USB_BUS: Option<bus::UsbBusAllocator<UsbBusType>> = None;

        // Get access to the device specific peripherals from the peripheral access crate
        let mut peripherals = c.device;
        // Take ownership over the raw flash and rcc devices and convert them into the corresponding
        // HAL structs
        let rcc = peripherals.RCC.constrain();

        // Freeze the configuration of all the clocks in the system and store the frozen frequencies in
        // `clocks`
        let clocks = rcc.cfgr
            .use_hse(25.mhz())
            .sysclk(48.mhz())
            .require_pll48clk()
            .freeze();

        // Prepare the GPIO peripherals
        let gpioa = peripherals.GPIOA.split();
        let gpiob = peripherals.GPIOB.split();
        let gpioc = peripherals.GPIOC.split();
 
        let mut led =  gpioc.pc13.into_push_pull_output();
        led.set_low().unwrap();

        
        let usb = USB {
            usb_global: peripherals.OTG_FS_GLOBAL,
            usb_device: peripherals.OTG_FS_DEVICE,
            usb_pwrclk: peripherals.OTG_FS_PWRCLK,
            pin_dm: gpioa.pa11.into_alternate_af10(),
            pin_dp: gpioa.pa12.into_alternate_af10(),
        };
 
        *USB_BUS = Some(UsbBus::new(usb, singleton!(: [u32; 1024] = [0; 1024]).unwrap()));
  
        let mut usb_serial = ConsoleSerial(usbd_serial::SerialPort::new(USB_BUS.as_ref().unwrap()));
    
        let usb_dev = UsbDeviceBuilder::new(USB_BUS.as_ref().unwrap(), UsbVidPid(0x16c0, 0x27dd))
            .manufacturer("Fake company")
            .product("NRF24L01+ receiver")
            .serial_number("TEST")
            .device_class(usbd_serial::USB_CLASS_CDC)
            .build();
        
        writeln!(usb_serial, "Starting").unwrap();
     
        let ce = gpiob.pb0.into_push_pull_output();
        let csn = gpiob.pb1.into_push_pull_output();

        let spi = Spi::spi1(
            peripherals.SPI1,
            (
                gpioa.pa5.into_alternate_af5(),
                gpioa.pa6.into_alternate_af5(),
                gpioa.pa7.into_alternate_af5(),
            ),
            Mode {
                polarity: Polarity::IdleLow,
                phase: Phase::CaptureOnFirstTransition
            },
            1000000.hz(),
            clocks,
        );

        
        let radio = match NRF24L01::new(ce, csn, spi) {
            Ok(radio) => {
                match radio.rx() {
                    Ok(rx) => {
                        writeln!(usb_serial, "Radio initialized").unwrap();
                        Some(rx)
                    },
                    Err(_) => {
                        writeln!(usb_serial, "RX mode failed").unwrap();
                        None
                    }
                }
            },
            Err(_) => {
                writeln!(usb_serial, "INIT failed").unwrap();
                None
            }
        };
        
        let mut irq = gpiob.pb10.into_pull_up_input();
        irq.make_interrupt_source(&mut peripherals.SYSCFG);
        irq.trigger_on_edge(&mut peripherals.EXTI, Edge::FALLING);
        irq.enable_interrupt(&mut peripherals.EXTI);

        // Configure the syst timer to trigger an update every second and enables interrupt
        let mut timer = Timer::tim2(peripherals.TIM2, 1.hz(), clocks);
        timer.listen(Event::TimeOut);

        init::LateResources {
            radio,
            irq: irq,
            values: [0; 4],
            usb_dev,
            usb_serial,
            timer,
            led,
 		}
    }

    #[task(binds = EXTI15_10, priority = 1, 
        resources = [ irq ],
        spawn = [ receive ])]
    fn interrupt(c: interrupt::Context) {
        if c.resources.irq.is_low().unwrap() {
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

    #[task(resources=[values,  usb_serial ])]
    fn process(c: process::Context, payload: Payload) {
        let result : postcard::Result<Transmitter> = postcard::from_bytes(&payload);
        match result {
            Ok(Transmitter { correlation_id, body }) => {
                writeln!(c.resources.usb_serial, "Received message {}", correlation_id).unwrap();
                match body {
                    ChannelValues(values) => {
                        *c.resources.values = values;
                    }
                }
            },

            Err(_) => {}

        }
    }

    #[task(binds = TIM2, priority = 1, resources = [ timer, led, usb_serial ])]
    fn tick(c: tick::Context) {
        c.resources.timer.clear_interrupt(Event::TimeOut);
        c.resources.led.toggle().unwrap();
        writeln!(c.resources.usb_serial, "Tick").unwrap();
    }

    
    #[task(binds = OTG_FS, resources = [usb_dev, usb_serial])]
    fn otg_fs(c: otg_fs::Context) {
        c.resources.usb_dev.poll(&mut [&mut c.resources.usb_serial.0]);
    }
    
    extern "C" {
        fn USART2();
    }
};

