package ActionsValidation {
    action def ProcessData;
    action def ReadSensor;
    part def Controller {
        action process : ProcessData;
        action read : ReadSensor;
    }
}
