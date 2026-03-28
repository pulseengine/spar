// SysML v2 Validation: Ref usages
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package RefUsagesValidation {

    part def Person {
        attribute name;
    }

    part def Vehicle {
        ref part driver : Person;
        ref part owner : Person;
    }

    part def Fleet {
        part vehicles [0..*] : Vehicle;
        ref part manager : Person;
    }
}
