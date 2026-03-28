package PartsValidation {
    part def Engine {
        part cylinder : Cylinder;
    }
    part def Cylinder;
    part def Vehicle {
        part eng : Engine;
        part body : Body;
    }
    part def Body;
}
