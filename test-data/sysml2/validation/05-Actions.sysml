// SysML v2 Validation: Actions
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package ActionsValidation {

    action def SenseAction {
        in item rawData;
        out item processedData;
    }

    action def ProcessAction {
        in item inputData;
        out item result;
    }

    action def ActuateAction {
        in item command;
    }

    part def ControlSystem {
        action sense : SenseAction;
        action process : ProcessAction;
        action actuate : ActuateAction;
    }
}
