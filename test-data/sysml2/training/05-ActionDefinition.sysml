// SysML v2 Training: Action Definitions
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/05-*

package ActionDefinitionTraining {

    action def Sense {
        in item environment;
        out item sensorData;
    }

    action def Plan {
        in item sensorData;
        out item actionPlan;
    }

    action def Act {
        in item actionPlan;
        out item actuatorCommand;
    }

    part def AutonomousSystem {
        action sense : Sense;
        action plan : Plan;
        action act : Act;
    }
}
