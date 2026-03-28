package AttributesValidation {
    attribute def Mass;
    attribute def Voltage;
    part def Sensor {
        attribute mass : Mass;
        attribute voltage : Voltage;
    }
}
