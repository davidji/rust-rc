#![deny(unsafe_code)]
#![deny(warnings)]
#![no_main]
#![no_std]

extern crate panic_semihosting;
extern crate nb;

use cortex_m::{ singleton };

use core::{
    default::Default,
	option::Option,
    convert::Infallible,
    fmt::{ self, Write },
};

use embedded_nrf24l01::{
    NRF24L01, RxMode, Payload, Configuration, DataRate, CrcMode
};

use stm32f1xx_hal::{
	self,
    prelude::*,
    gpio::{ AF5, Alternate,  Edge, ExtiPin, Input, Output, PullUp, PushPull },
    gpio::{ 
        gpioa::{ 
            PA5,  // SCLK
            PA6,  // MISO
            PA7,  // MOSI
            PA4,  // CSN
            // PA9,  // TX1: SUMD to flight controller
            // PA10, // RX1: telemetry from flight controller
         },
        gpiob::{ 
            PB0,  // CE
            PB10, // IRQ: Note: if you change this pin you must change the EXTI interrupt below
        },
        gpioc::{
            PC13
        }
    },
    otg_fs::{ USB, UsbBus, UsbBusType },
    serial::{ 
        self,
        Serial,
        Tx,
    },
    spi::{ Mode, Phase, Polarity, Spi },
    stm32::{ SPI1, TIM2, USART1 },
    timer::{ Timer, Event },
    
};


use usb_device::{
    bus,
    prelude::*
};

use usbd_serial;

use protocol::{ 
    Transmitter, 
    TransmitterMessage::*,
};

use sumd::{ self, Sumd };

use postcard;

type RadioCe = PB0<Output<PushPull>>;
type RadioCsn = PA4<Output<PushPull>>;
type RadioIrq = PB10<Input<PullUp>>;


type RadioSpi = Spi<SPI1,
    (PA5<Alternate<AF5>>, 
     PA6<Alternate<AF5>>, 
     PA7<Alternate<AF5>>), u8>;

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

#[derive(Debug)]
pub enum InitStatus {
    Ok,
    RadioInitFailed,
    RadioReceiveFailed
}

pub struct Status {
    init: InitStatus,
    counter: u32,
    last_correlation_id: u32,
    missed_messages: u32,
    values: [protocol::Value; 4],
    interrupts: u32,
    missed_interrupts: u32,
}

fn sumd_serial_config() -> serial::config::Config {
    let default : serial::config::Config = Default::default();
    default.baudrate(115200.bps())
}

// type FlightControllerSerial = Serial<USART1, (PA9<Alternate<AF7>>, PA10<Alternate<AF7>>)>;

#[rtfm::app(device = stm32f4::stm32f411, peripherals=true)]
const APP: () = {

    struct Resources {
        radio: Option<RxMode<Radio>>,
        irq: RadioIrq,
        status: Status,
        usb_dev: UsbDevice<'static, UsbBusType>,
        usb_serial: ConsoleSerial,
        timer: Timer<TIM2>,
        led: PC13<Output<PushPull>>,
        flight_controller: Sumd<Tx<USART1>>,
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
  
        let usb_serial = ConsoleSerial(usbd_serial::SerialPort::new(USB_BUS.as_ref().unwrap()));
    
        let usb_dev = UsbDeviceBuilder::new(USB_BUS.as_ref().unwrap(), UsbVidPid(0x16c0, 0x27dd))
            .manufacturer("Fake company")
            .product("NRF24L01+ receiver")
            .serial_number("TEST")
            .device_class(usbd_serial::USB_CLASS_CDC)
            .build();

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

        let mut irq = gpiob.pb10.into_pull_up_input();
        irq.make_interrupt_source(&mut peripherals.SYSCFG);
        irq.trigger_on_edge(&mut peripherals.EXTI, Edge::FALLING);
        irq.enable_interrupt(&mut peripherals.EXTI);

        let (radio, status) = match NRF24L01::new(ce, csn, spi) {
            Ok(mut radio) => {
                radio.set_frequency(protocol::FREQUENCY).unwrap();

                radio.set_rf(DataRate::R250Kbps, 0).unwrap();
                radio.set_crc(Some(CrcMode::TwoBytes)).unwrap();
                radio.set_auto_ack(&[ true; 6 ]).unwrap();
                radio.set_auto_retransmit(0b0100, 15).unwrap();

                radio.set_rx_addr(0, &protocol::RX_ADDRESS).unwrap();
                radio.set_pipes_rx_lengths(&[ None; 6]).unwrap();
                radio.set_pipes_rx_enable(&[true, false, false, false, false, false]).unwrap();
                radio.flush_tx().unwrap();
                radio.flush_rx().unwrap();

                radio.set_interrupt_mask(true, true, true).unwrap();
                radio.set_interrupt_mask(false, false, false).unwrap();
                radio.clear_interrupts().unwrap();

                match radio.rx() {
                    Ok(rx) => {
                        (Some(rx), InitStatus::Ok)
                    },
                    Err(_) => {
                        (None, InitStatus::RadioReceiveFailed)
                    }
                }
            },
            Err(_) => {
                (None, InitStatus::RadioInitFailed)
            }
        };
        
        

        let flight_controller = Serial::usart1(
            peripherals.USART1, 
            (gpioa.pa9.into_alternate_af7(), gpioa.pa10.into_alternate_af7()),
            sumd_serial_config(),
            clocks).unwrap();

        let (flight_controller_tx, _) = flight_controller.split();

        // Configure the syst timer to trigger an update every second and enables interrupt
        let mut timer = Timer::tim2(peripherals.TIM2, 100.hz(), clocks);
        timer.listen(Event::TimeOut);

        init::LateResources {
            radio,
            irq: irq,
            status: Status {
                init: status,
                values: [0; 4],
                counter: 0,
                last_correlation_id: 0,
                missed_messages: 0,
                interrupts: 0,
                missed_interrupts: 0,
            },
            usb_dev,
            usb_serial,
            timer,
            led,
            flight_controller: Sumd::new(flight_controller_tx),
 		}
    }

    #[task(binds = EXTI15_10, priority = 1, 
        resources = [ irq, status ],
        spawn = [ receive ])]
    fn interrupt(c: interrupt::Context) {
            c.resources.status.interrupts += 1;
            c.resources.irq.clear_interrupt_pending_bit();
            match c.spawn.receive() {
                Ok(_) => {},
                Err(_) => {
                    c.resources.status.missed_interrupts += 1;
                }
            };
    }

    #[task(resources = [radio, status], spawn=[process])]
    fn receive(c: receive::Context) {
        let mut rx = c.resources.radio.take().unwrap();
        rx.clear_interrupts().unwrap();
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
        *c.resources.radio = Some(rx);
    }

    #[task(resources=[status])]
    fn process(c: process::Context, payload: Payload) {
        let result : postcard::Result<Transmitter> = postcard::from_bytes(&payload);
        match result {
            Ok(Transmitter { correlation_id, body }) => {
                c.resources.status.missed_messages += 
                    correlation_id - c.resources.status.last_correlation_id - 1;
                c.resources.status.last_correlation_id = correlation_id;
                match body {
                    ChannelValues(values) => {
                        c.resources.status.values = values;
                    }
                }
            },

            Err(_) => {}

        }
    }

    #[task(resources = [status, usb_serial])]
    fn log_status(c: log_status::Context, can_read: bool, is_full: bool) {
        let _ = writeln!(c.resources.usb_serial, 
            "Tick; init: {:?} last(missed): {}({}), interrupts(missed): {}({}), can_read: {}, is_full: {}",
            c.resources.status.init,
            c.resources.status.last_correlation_id,
            c.resources.status.missed_messages,
            c.resources.status.interrupts,
            c.resources.status.missed_interrupts,
            can_read, 
            is_full);
        let _ = writeln!(c.resources.usb_serial, "channels: {:?}", c.resources.status.values);
    }

    #[task(binds = TIM2, priority = 1, resources = [ status, timer, led, radio ], 
        spawn = [ log_status, receive, send_to_flight_controller ])]
    fn tick(c: tick::Context) {
        c.resources.timer.clear_interrupt(Event::TimeOut);

        let mut rx = c.resources.radio.take().unwrap();
        let can_read = rx.can_read().unwrap().is_some();
        if can_read {
            c.spawn.receive().unwrap();
        }

        if c.resources.status.counter % 10 == 0 {
            c.spawn.send_to_flight_controller(c.resources.status.values).unwrap();
        }

        if c.resources.status.counter % 500 == 0 {
            c.resources.led.toggle().unwrap();
        }

        if c.resources.status.counter % 1000 == 0 {
            c.spawn.log_status(can_read, rx.is_full().unwrap()).unwrap();
        }


        c.resources.status.counter += 1;

        *c.resources.radio = Some(rx);
    }

    #[task(resources = [flight_controller, status])]
    fn send_to_flight_controller(c: send_to_flight_controller::Context, values: [protocol::Value; 4]) {
        c.resources.flight_controller.send(sumd::Status::Live, &values).unwrap();
    }
    
    #[task(binds = OTG_FS, resources = [usb_dev, usb_serial])]
    fn otg_fs(c: otg_fs::Context) {
        c.resources.usb_dev.poll(&mut [&mut c.resources.usb_serial.0]);
    }
    
    extern "C" {
        fn USART2();
        fn USART3();
    }
};

