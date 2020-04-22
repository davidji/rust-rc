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

use cortex_m::{ singleton };

use protocol;
use postcard;
use embedded_nrf24l01::{
    NRF24L01, StandbyMode
};

use stm32f1xx_hal::{
	self,
    prelude::*,
    adc::{ self, Adc, AdcDma, Scan, SetChannels },
    pac,
    gpio::{ 
        Alternate, Analog, Floating, Input, Output, PushPull, State,
        gpioa::{ PA0, PA1, PA2, PA3, PA5, PA6, PA7},
        gpiob::{ 
            PB12, // LED 
            PB0, // CE
            PB1 // CSN
        },
    },
    spi::{ Mode, Phase, Polarity, Spi, Spi1NoRemap },
    stm32::{ ADC1, SPI1 },
    timer::{ Timer, CountDownTimer, Event },
};

type RadioCe = PB0<Output<PushPull>>;
type RadioCsn = PB1<Output<PushPull>>;

type RadioSpi = Spi<SPI1, Spi1NoRemap, 
    (PA5<Alternate<PushPull>>, 
     PA6<Input<Floating>>, 
     PA7<Alternate<PushPull>>)>;

type Radio = NRF24L01<Infallible, RadioCe, RadioCsn, RadioSpi>;

pub struct Counter {
    correlation_id: i32,
}

pub struct JoystickAdcPins(PA0<Analog>, PA1<Analog>, PA2<Analog>, PA3<Analog>);

impl SetChannels<JoystickAdcPins> for Adc<ADC1> {
    fn set_samples(&mut self) {
        self.set_channel_sample_time(0, adc::SampleTime::T_28);
        self.set_channel_sample_time(1, adc::SampleTime::T_28);
        self.set_channel_sample_time(2, adc::SampleTime::T_28);
        self.set_channel_sample_time(3, adc::SampleTime::T_28);
    }

    fn set_sequence(&mut self) {
        self.set_regular_sequence(&[0, 1, 2, 3]);
    }
}

#[rtfm::app(device = stm32f1xx_hal::pac, peripherals=true)]
const APP: () = {
    struct Resources {
        radio: Option<StandbyMode<Radio>>,
        counter: Counter,
        joystick_scan: Option<(AdcDma<JoystickAdcPins, Scan>,&'static mut [u16; 4])>,
        timer: CountDownTimer<pac::TIM1>,
        led: PB12<Output<PushPull>>
    }

    #[init]
    fn init(cx: init::Context) -> init::LateResources {
        // Take ownership over the raw flash and rcc devices and convert them into the corresponding
        // HAL structs
        let mut flash = cx.device.FLASH.constrain();
        let mut rcc = cx.device.RCC.constrain();

        // Freeze the configuration of all the clocks in the system and store the frozen frequencies in
        // `clocks`
        let clocks = rcc.cfgr.use_hse(8.mhz()).freeze(&mut flash.acr);

        // Prepare the alternate function I/O registers
        let mut afio = cx.device.AFIO.constrain(&mut rcc.apb2);

        let mut gpiob = cx.device.GPIOB.split(&mut rcc.apb2);
        let led = gpiob.pb12.into_push_pull_output_with_state(&mut gpiob.crh, State::Low);

        // Prepare the GPIO peripherals
        let mut gpioa = cx.device.GPIOA.split(&mut rcc.apb2);
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
            cx.device.SPI1,
            spi_pins,
            &mut afio.mapr,
            spi_mode,
            1.mhz(),
            clocks,
            &mut rcc.apb2
        );

        let radio = NRF24L01::new(ce, csn, spi).unwrap();

	    let joystick_adc = adc::Adc::adc1(cx.device.ADC1, &mut rcc.apb2, clocks);
    	let joystick_channels = JoystickAdcPins(
        	gpioa.pa0.into_analog(&mut gpioa.crl),
        	gpioa.pa1.into_analog(&mut gpioa.crl),
        	gpioa.pa2.into_analog(&mut gpioa.crl),
        	gpioa.pa3.into_analog(&mut gpioa.crl)
    	);

	    let dma_ch1 = cx.device.DMA1.split(&mut rcc.ahb).1;
		let joystick_scan = joystick_adc.with_scan_dma(joystick_channels, dma_ch1);
        
        // Configure the syst timer to trigger an update every milli second and enables interrupt
        let mut timer = Timer::tim1(cx.device.TIM1, &clocks, &mut rcc.apb2).start_count_down(1.khz());
        timer.listen(Event::Update);

        init::LateResources { 
            radio: Some(radio),
            counter: Counter { correlation_id: 0 },
            joystick_scan: Some((joystick_scan, singleton!(: [u16; 4] = [0; 4]).unwrap())),
            timer: timer,
            led: led,
        }
    }

    #[task(binds = TIM1_UP, priority = 1, 
        resources = [ joystick_scan, timer ],
        spawn = [ transmit ])]
    fn update(c: update::Context) {
	   	let (joystick_scan, dma_buffer) = c.resources.joystick_scan.take().unwrap();
        let (dma_buffer, joystick_scan) = joystick_scan.read(dma_buffer).wait();
        c.spawn.transmit(protocol::TransmitterMessage::ChannelValues(*dma_buffer)).unwrap();
        *c.resources.joystick_scan = Some((joystick_scan, dma_buffer));
        c.resources.timer.clear_update_interrupt_flag();
    }

    #[task(resources = [ radio, counter, led ])]
    fn transmit(c: transmit::Context, body: protocol::TransmitterMessage) {
        if c.resources.counter.correlation_id % 500 == 0 {
            c.resources.led.toggle().unwrap();
        }
		let message = protocol::Transmitter { correlation_id: c.resources.counter.correlation_id, body };
		c.resources.counter.correlation_id += 1;
		let standby = c.resources.radio.take().unwrap();
    	let mut tx = standby.tx().unwrap();
		let mut buf = [0u8; 32];
		tx.send(postcard::to_slice(&message, &mut buf).unwrap()).unwrap();
		tx.wait_empty().unwrap();
        *c.resources.radio = Some(tx.standby().unwrap());
    }

    extern "C" {
        fn USART2();
    }
};
