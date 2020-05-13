#![no_std]
pub mod sumd;
use serde::{Serialize, Deserialize};

pub type Value = u16;
pub type Channel = u16;

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub enum TransmitterMessage {
    ChannelValues([Value; 4])
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct Transmitter {
    pub correlation_id : u32,
    pub body : TransmitterMessage

}

pub const FREQUENCY : u8 = 76;
pub const TX_ADDRESS : [u8;5] = [ 'R' as u8, 'C' as u8, 'T' as u8, 'X' as u8, 0x00 ];
pub const RX_ADDRESS : [u8;5] = [ 'R' as u8, 'C' as u8, 'R' as u8, 'X' as u8, 0x00 ];

 #[test]
 fn fits() {
    use postcard;
    let mut buf = [0u8; 32];
    let message = Transmitter { correlation_id: 0, body: TransmitterMessage::ChannelValues([0; 4])};
    postcard::to_slice(&message, &mut buf).unwrap();
}
