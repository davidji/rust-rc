
#include <Arduino.h>

#include <NRF24.h>
#include <SPI.h>


NRF24 nrf24(9, 10);
// NRF24 nrf24(8, 7); // use this to be electrically compatible with Mirf
// NRF24 nrf24(8, 10);// For Leonardo, need explicit SS pin

bool setupRadio() {
    if (!nrf24.init()) {
        Serial.println("NRF24 init failed");
        return false;
    }

    if (!nrf24.setChannel(76)) {
        Serial.println("setChannel failed");
        return false;
    }

    if (!nrf24.setThisAddress((uint8_t*)"RCRX\0", 5)) {
        Serial.println("setThisAddress failed");
        return false;
    }

    if (!nrf24.setRF(NRF24::NRF24DataRate250kbps, NRF24::NRF24TransmitPower0dBm)) {
        Serial.println("setRF failed");
        return false;
    }

    if (!nrf24.powerUpRx()) {
        Serial.println("powerOnRx failed");
        return false;
    }

    Serial.println("NRF24 Initialised");
    return true;
}

void setup() {
    Serial.begin(115200);
    while (!Serial); // wait for serial port to connect. Needed for Leonardo only
    setupRadio();
}

 int count_empty = 0;

void loop()
{
    uint8_t buf[32];
    uint8_t len = sizeof(buf);

    nrf24.waitAvailable();
    if (nrf24.recv(buf, &len)) {
        if(len > 0) {
            Serial.print("received packet ");
            Serial.print(len);
            Serial.println(" bytes");
        } else {
            count_empty++;
            if(count_empty %100 == 0) {
                Serial.print(count_empty);
                Serial.println(" empty packets");
            }
        }
        
    }
}
