// SysML v2 model for roundtrip integration test.
// Maps to the equivalent AADL BuildingControl model.
//
// Pipeline: SysML v2 → parse → lower → AADL text → parse AADL → instantiate → analyze → codegen

package BuildingControl {

    // Data types
    port def SensorPort {
        out item reading;
    }

    port def ActuatorPort {
        in item command;
    }

    // Hardware: processor
    part def CPU { }

    // Software: controller process with ports
    part def Controller {
        in port sensorIn : SensorPort;
        out port commandOut : ActuatorPort;
        part ctrlThread : ControlLoop;
    }

    // Thread with action (software behavior)
    part def ControlLoop {
        in port sensorData : SensorPort;
        out port command : ActuatorPort;
        action processData { }
    }

    // Top-level system: composes hardware + software
    part def BuildingSystem {
        part cpu : CPU;
        part controller : Controller;
        connect controller.sensorIn to controller.commandOut;
    }
}
