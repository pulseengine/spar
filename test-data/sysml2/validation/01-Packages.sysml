// SysML v2 Validation: Packages
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package PackagesExample {
    doc /* This is a package example from the official validation suite. */

    package InnerPkg {
        part def A { }
    }

    package OuterPkg {
        import InnerPkg::*;
        part def B { }
    }
}
