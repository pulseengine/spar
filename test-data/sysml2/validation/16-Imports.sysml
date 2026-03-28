package ImportSource {
    part def Sensor;
}
package ImportTarget {
    import ImportSource::*;
    part def System {
        part s : Sensor;
    }
}
