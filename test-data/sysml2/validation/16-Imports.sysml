// SysML v2 Validation: Imports
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package ImportBase {
    part def Alpha { }
    part def Beta { }

    package Nested {
        part def Gamma { }
    }
}

package ImportTarget {
    import ImportBase::*;
    import ImportBase::Nested::Gamma;

    part myAlpha : Alpha;
    part myBeta : Beta;
    part myGamma : Gamma;
}
