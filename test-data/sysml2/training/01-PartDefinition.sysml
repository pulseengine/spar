// SysML v2 Training: Part Definitions
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/01-*

package PartDefinitionTraining {

    part def Vehicle {
        doc "A basic vehicle definition"
    }

    part def Engine {
        attribute horsePower : Real;
        attribute displacement : Real;
    }

    part def Body {
        attribute color;
        attribute material;
    }

    part def Chassis {
        part engine : Engine;
        part body : Body;
    }
}
