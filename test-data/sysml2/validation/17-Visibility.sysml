// SysML v2 Validation: Visibility (public/private)
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package VisibilityValidation {
    private import ScalarValues::*;
    public import Definitions::*;

    part def PublicPart {
        doc "Visible to all importers"
    }

    part def InternalPart {
        doc "Internal implementation detail"
    }

    part def System {
        part publicComponent : PublicPart;
        part internalComponent : InternalPart;
    }
}
