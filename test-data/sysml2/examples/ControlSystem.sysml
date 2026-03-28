// SysML v2 Example: Control System
// Exercises actions, states, requirements, and constraints together

package ControlSystemExample {
    doc /* A flight control system model. */

    // Attribute definitions
    attribute def Angle;
    attribute def Rate;
    attribute def Force;

    // Port definitions
    port def SensorIF {
        out item angle : Angle;
        out item rate : Rate;
    }

    port def ActuatorIF {
        in item force : Force;
    }

    port def CommandIF {
        in item targetAngle : Angle;
    }

    // Part definitions
    part def IMU {
        doc "Inertial Measurement Unit"
        port sensorOut : SensorIF;
        attribute biasStability : Real;
    }

    part def Actuator {
        doc "Control surface actuator"
        port actuatorIn : ActuatorIF;
        attribute maxForce : Real;
        attribute bandwidth : Real;
    }

    part def FlightComputer {
        doc "Flight control computer"
        port sensorIn : SensorIF;
        port actuatorOut : ActuatorIF;
        port commandIn : CommandIF;
        attribute processingRate : Real = 400;
    }

    // Action definitions
    action def SensorFusion {
        doc "Fuse multiple sensor inputs"
        in item rawSensorData;
        out item fusedData;
    }

    action def ControlLaw {
        doc "Execute control law"
        in item fusedData;
        in item command;
        out item actuatorCommand;
    }

    // State definitions
    state def FlightModes {
        doc "Flight control modes"
    }

    // Constraint definitions
    constraint def StabilityConstraint {
        doc "System must maintain stability margins"
    }

    constraint def LatencyConstraint {
        doc "Control loop latency must be bounded"
    }

    // System composition
    part def FlightControlSystem {
        doc "Complete flight control system"

        part imu [3] : IMU;
        part actuator [4] : Actuator;
        part computer [2] : FlightComputer;

        action sensorFusion : SensorFusion;
        action controlLaw : ControlLaw;
        state mode : FlightModes;

        constraint stability : StabilityConstraint;
        constraint latency : LatencyConstraint;

        // Connections
        connect imu.sensorOut to computer.sensorIn;
        connect computer.actuatorOut to actuator.actuatorIn;
    }

    // Requirements
    requirement def FCSReq {
        doc "Flight control system requirements"
    }

    requirement stabilityReq : FCSReq {
        doc "System must maintain 6dB gain margin and 45deg phase margin"
    }

    requirement redundancyReq : FCSReq {
        doc "System must tolerate dual sensor failure"
    }

    requirement latencyReq : FCSReq {
        doc "Control loop latency must be below 2.5ms"
    }

    // Traceability
    satisfy stabilityReq by FlightControlSystem;
    satisfy redundancyReq by FlightControlSystem;
    verify latencyReq by FlightControlSystem;
    refine stabilityReq by latencyReq;
}
