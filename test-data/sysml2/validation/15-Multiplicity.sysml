package MultiplicityValidation {
    part def Wheel;
    part def Engine;
    part def Vehicle {
        part wheels [4] : Wheel;
        part engine [1] : Engine;
    }
}
