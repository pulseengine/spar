package RequirementsValidation {
    requirement def LatencyReq {
        doc "System shall respond within 100ms";
        subject s : Controller;
    }
    part def Controller;
    requirement sysLatency : LatencyReq;
    satisfy sysLatency by Controller;
}
