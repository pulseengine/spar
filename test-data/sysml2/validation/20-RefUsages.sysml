package RefUsagesValidation {
    part def Sensor;
    part def Controller {
        ref part mySensor : Sensor;
    }
}
