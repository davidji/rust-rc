#![no_std]
use serde::{Serialize, Deserialize};

pub type Value = u16;
pub type Channel = u16;

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub enum TransmitterMessage {
    ChannelValues([Value; 4])
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct Transmitter {
    pub correlation_id : i32,
    pub body : TransmitterMessage

}
