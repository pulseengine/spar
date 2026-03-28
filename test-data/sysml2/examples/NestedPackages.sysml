// SysML v2 Example: Nested Packages
// Tests deep package nesting and cross-package references

package TopLevel {
    doc /* Top-level package with nested structure. */

    package Definitions {
        attribute def Mass;
        attribute def Length;
        attribute def Time;

        part def Component {
            attribute mass : Mass;
        }

        package Mechanical {
            part def Structure :> Component {
                attribute length : Length;
            }

            part def Mechanism :> Component {
                attribute range : Length;
            }
        }

        package Electrical {
            part def Circuit :> Component {
                attribute voltage : Real;
            }

            part def Sensor :> Component {
                attribute sampleRate : Real;
            }
        }

        package Software {
            part def Process {
                attribute period : Time;
            }

            part def Thread :> Process {
                attribute priority : Real;
            }
        }
    }

    package Usages {
        import Definitions::*;
        import Definitions::Mechanical::*;
        import Definitions::Electrical::*;
        import Definitions::Software::*;

        part myStructure : Structure;
        part myCircuit : Circuit;
        part myThread : Thread;
    }

    package System {
        import Definitions::*;

        part def TopSystem {
            part structure : Definitions::Mechanical::Structure;
            part circuit : Definitions::Electrical::Circuit;
            part controller : Definitions::Software::Thread;
        }
    }
}
