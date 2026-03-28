// SysML v2 Validation: Allocations
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package AllocationsValidation {

    allocation def TaskAllocation {
        doc "Software task to hardware allocation"
    }

    allocation def MemoryAllocation {
        doc "Data to memory region allocation"
    }

    part def SoftwareTask;
    part def HardwareProcessor;
    part def DataBuffer;
    part def MemoryRegion;

    part def System {
        part swTask : SoftwareTask;
        part hwProcessor : HardwareProcessor;
        part dataBuffer : DataBuffer;
        part memory : MemoryRegion;
    }
}
