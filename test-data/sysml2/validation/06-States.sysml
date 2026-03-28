package StatesValidation {
    state def Operational;
    state def Degraded;
    part def Controller {
        state operational : Operational;
        state degraded : Degraded;
    }
}
