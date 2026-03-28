package ItemsValidation {
    item def SensorData;
    item def CommandData;
    part def Sensor {
        out item reading : SensorData;
    }
    part def Actuator {
        in item command : CommandData;
    }
}
