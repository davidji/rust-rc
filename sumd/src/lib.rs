// This is based on this document:
// https://www.deviationtx.com/media/kunena/attachments/98/HoTT-SUMD-Spec-REV01-12062012-pdf.pdf
#![no_std]

use core::{
    convert::From,
    u16
};

use nb;
use embedded_hal::serial::Write;
use heapless::{ consts::*, Vec };

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
pub const OFFSET: u16 = NEUTRAL - HIGH/2;
pub const SCALE_EXTENDED: u16 = u16::MAX/(EXTENDED_HIGH - EXTENDED_LOW);
pub const OFFSET_EXTENDED: u16 = NEUTRAL - EXTENDED_HIGH/2;

pub fn scale(value: u16) -> u16 {
    value/SCALE + OFFSET
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


pub fn send<Out: Write<u8>>(out: &mut Out, status: Status, values : &[u16]) -> nb::Result<(), Out::Error> {
    let mut crc = Crc16(0);

    let mut write = |c| {
        crc.update(c);
        out.write(c)
    };

    write(VENDOR_ID)?;
    write(status as u8)?;
    write(values.len() as u8)?;
    for value in values {
        for byte in &scale(*value).to_be_bytes() {
            write(*byte)?;
        }
    }
    
    for byte in &crc.0.to_be_bytes() {
        out.write(*byte)?;
    }
    
    out.flush()?;
    Ok(())
}

pub struct SumdBuffer(pub Vec<u8, U69>);

impl Write<u8> for SumdBuffer {
    type Error = u8;

    /// Writes a single word to the serial interface
    fn write(&mut self, word: u8) -> nb::Result<(), Self::Error> {
        self.0.push(word)?;
        Ok(())
    }

    /// Ensures that none of the previously written words are still buffered
    fn flush(&mut self) -> nb::Result<(), Self::Error> {
        Ok(())
    }
}

impl SumdBuffer {
    pub fn new() -> Self {
        Self(Vec::new())   
    }
    
    pub fn encode(&mut self, status: Status, values: &[u16]) {
        send(self, status, values).unwrap();
    }

}


// The whole message then, has a maximum size of 3 + 32*2 + 2 = 69 bytes.
// It is transmitted on a 115200 baud serial link, 8N1, so the byte rate is
// 12800B/s, so it takes ~5.4mS to transmit a packet.
// generally, you would send a packet every 10mS.

