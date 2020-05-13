// This is based on this document:
// https://www.deviationtx.com/media/kunena/attachments/98/HoTT-SUMD-Spec-REV01-12062012-pdf.pdf


use core::{
    convert::From,
    result::Result,
    u16
};

use nb;
use embedded_hal::serial::Write;

// Each packet starts with the vendor id
const VENDOR_ID : u8 = 0xa8;

// Then a status
pub enum Status {
    Live,
    FailSafe
}

// encoded thus:
impl From<Status> for u8 {
    fn from(status: Status) -> u8 {
        match status {
            Status::Live => 0x01,
            Status::FailSafe => 0x81,
        }
    }
}

// Then a byte specifying the number of channels, up to 32
// Then the values themselves, which are unsigned 16 bits each, in network order, i.e. big-endian
// There are some reference values

/// Extended low position (-150%), equivalent to 900uS pulse length
pub const EXTENDED_LOW : u16 = 0x1c20;
/// Low (-100%), equivalent to 1100uS pulse length
pub const LOW: u16 = 0x2260;
/// Neutral position (0%), equivalent to 1500uS pulse length 
pub const NEUTRAL: u16 = 0x2ee0;
// High position (100%), equivalent to 1900µs pulse length
pub const HIGH: u16 = 0x3b60;
// Extended high position (+150%), equivalent to 2100µs pulse length
pub const EXTENDED_HIGH: u16 = 0x41a0;

pub const SCALE: u16 = u16::MAX/(HIGH - LOW);
pub const OFFSET: u16 = u16::MAX - NEUTRAL;

pub fn normalise(value: u16) -> u16 {
    value/SCALE - OFFSET
}

// Finally a 16 bit CRC, of all the bytes preceding it.
// This is defined in C in the specification, and this is a translation:
struct Crc16(u16);

impl Crc16 {
    const CRC_POLYNOME : u16 = 0x1021;

    pub fn update(&mut self, value: u8 ) {
        let mut crc = self.0;
        crc = crc ^ (value as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ Self::CRC_POLYNOME;
            } else {
                crc = crc << 1;
            }
        }

        self.0 = crc; 
    }
}

impl Default for Crc16 {
    fn default() -> Self { Crc16(0) } 
}

pub struct Sumd<Out>
where Out: Write<u8> {
    crc: Crc16,
    out: Out,
}

impl <Out: Write<u8>> Write<u8> for Sumd<Out> {
    type Error = Out::Error;

    fn write(&mut self, word: u8) -> Result<(), nb::Error<Self::Error>> {
        self.crc.update(word);
        self.out.write(word)
    }

    fn flush(&mut self) -> Result<(), nb::Error<Self::Error>> {
        self.out.flush()
    }
}

impl <Out: Write<u8>> Sumd<Out> {
    
    pub fn new(out: Out) -> Self {
        Self { crc: Default::default(), out }
    }

    pub fn send(&mut self, status: Status, values : &[u16]) -> Result<(), nb::Error<Out::Error>> {
        self.crc.0 = 0;
        self.write(VENDOR_ID)?;
        self.write(status as u8)?;
        self.write(values.len() as u8)?;
        for value in values {
            for byte in &normalise(*value).to_be_bytes() {
                self.write(*byte)?;
            }
        }
    
        for byte in &self.crc.0.to_be_bytes() {
            self.out.write(*byte)?;
        }
    
        Ok(())
    }
}

// The whole message then, has a maximum size of 3 + 32*2 + 2 = 69 bytes.
// It is transmitted on a 115200 baud serial link, 8N1, so the byte rate is
// 12800B/s, so it takes ~5.4mS to transmit a packet.
// generally, you would send a packet every 10mS.

