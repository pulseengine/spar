// SysML v2 Example: Edge Cases
// Tests parser robustness with various tricky constructs

package EdgeCases {

    // Empty package with semicolon form
    package EmptyPkg;

    // Package with only imports
    package ImportsOnly {
        import EdgeCases::*;
    }

    // Deeply nested definitions
    part def Level0 {
        part def Level1 {
            part def Level2 {
                attribute depth : Real = 3;
            }
        }
    }

    // Multiple specialization forms
    part def Base {
        attribute x : Real;
    }
    part def Child1 :> Base { }
    part def Child2 specializes Base { }

    // Multiplicity variations
    part single [1] : Base;
    part optional [0..1] : Base;
    part many [0..*] : Base;
    part atLeastOne [1..*] : Base;
    part exactFive [5] : Base;
    part ranged [2..4] : Base;

    // Direction on different element kinds
    part def DirectionalParts {
        in port inPort;
        out port outPort;
        inout port bidiPort;
        in item inItem;
        out item outItem;
        inout item bidiItem;
        in attribute inAttr;
        out attribute outAttr;
    }

    // Qualified name references
    part qualRef : EdgeCases::Base;

    // Redefines and subsets
    part def Redefiner {
        part child redefines Base;
    }
    part def Subsetter {
        part child subsets Base;
    }
    part shorthand :>> Base;

    // Multiple members of same kind
    part def ManyParts {
        part a : Base;
        part b : Base;
        part c : Base;
        part d : Base;
        part e : Base;
    }

    // Abstract definitions
    abstract part def AbstractComponent { }
    abstract port def AbstractPort { }
    abstract action def AbstractAction { }

    // Requirement traceability chain
    requirement def TopReq { }
    requirement def DetailedReq :> TopReq { }
    requirement topReq1 : TopReq;
    requirement detailedReq1 : DetailedReq;
    part def Impl;
    part def TestSuite;
    satisfy topReq1 by Impl;
    verify topReq1 by TestSuite;
    refine topReq1 by detailedReq1;

    // Connection with qualified endpoints
    part def ConnectedSystem {
        part sender {
            port outPort;
        }
        part receiver {
            port inPort;
        }
        connect sender.outPort to receiver.inPort;
    }

    // Feature declarations
    part def WithFeatures {
        feature x : Real;
        feature y : Real;
        attribute name = "test";
        attribute count = 42;
        attribute ratio = 3.14;
        attribute flag = true;
        attribute other = false;
    }
}
