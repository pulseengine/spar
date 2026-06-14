# spar-network NC cross-validation oracle generator.
# Reproduces spar fixtures in panco (Bouillard reference impl) -> reference bounds.
# Units: bits + seconds throughout; printed in microseconds.
from panco.descriptor.curves import TokenBucket, RateLatency
from panco.descriptor.flow import Flow
from panco.descriptor.server import Server
from panco.descriptor.network import Network
from panco.fifo.tfaLP import TfaLP
from panco.fifo.sfaLP import SfaLP
from panco.fifo.fifoLP import FifoLP

GBPS=1e9; MBPS100=1e8; B=8.0; BIG=1e12
def srv(R,T): return Server([RateLatency(R,T)],[TokenBucket(0,BIG)])
def us(x): return round(x*1e6,4)
def bounds(net, label):
    tfa=TfaLP(net).all_delays
    sfa=SfaLP(net).all_delays
    # PURE polynomial PLP (no TFA strengthening): the only variant spar can
    # build byte-identically to this LP from its own model (REQ-NC-PLP-001).
    plp=FifoLP(net,polynomial=True,tfa=False).all_delays
    # TFA-STRENGTHENED PLP (tfa=True): embeds panco's per-server TFA delays in
    # the LP, tightening toward EXACT. spar's plp_bound_tfa_strengthened
    # (REQ-NC-PLP-003) reproduces this column using its OWN TFA delays, so the
    # match is to the pinned values under EXACT <= plp <= TFA, not LP identity.
    plp_str=FifoLP(net,polynomial=True,tfa=True).all_delays
    exact=FifoLP(net,polynomial=False).all_delays
    print(f"--- {label}")
    print(f"  TFA         : {[us(x) for x in tfa]}")
    print(f"  SFA         : {[us(x) for x in sfa]}")
    print(f"  PLP pure    : {[us(x) for x in plp]}")
    print(f"  PLP tfa=True: {[us(x) for x in plp_str]}")
    print(f"  EXACT       : {[us(x) for x in exact]}")

# 1) tandem_cross: tagged(0->1)+cross(0). spar TFA tagged=54.4, cross=34.0
bounds(Network([srv(GBPS,10e-6),srv(GBPS,5e-6)],
               [Flow([TokenBucket(1500*B,MBPS100)],[0,1]),
                Flow([TokenBucket(1500*B,MBPS100)],[0])]),
       "tandem_cross  (spar TFA: tagged 54.4us, cross 34.0us)")

# 2) single-flow tandem: spar D0=22, D1=19.2, e2e=41.2us
bounds(Network([srv(GBPS,10e-6),srv(GBPS,5e-6)],
               [Flow([TokenBucket(1500*B,MBPS100)],[0,1])]),
       "single_flow_tandem  (spar TFA e2e: 41.2us)")

# 3) 3-server line, tagged crosses all 3, two cross-flows
bounds(Network([srv(GBPS,10e-6),srv(GBPS,10e-6),srv(GBPS,10e-6)],
               [Flow([TokenBucket(1500*B,MBPS100)],[0,1,2]),
                Flow([TokenBucket(1500*B,MBPS100)],[0,1]),
                Flow([TokenBucket(1500*B,MBPS100)],[1,2])]),
       "three_server_line  (tagged 0->1->2 + 2 cross)")

# 4) converging bridge (wctt.rs::bridge_converge_aadl): two talkers merge at a
# shared sink. sw_a fast 5us [0], sw_b slow 10000us [1], sw_c fast 5us sink [2];
# dataA [0,2], dataB [1,2]. This is THE net that breaks pure PLP: dataA pure-PLP
# 1148.33 > TFA 1048.09 (both sound, incomparable). tfa=True drives it to EXACT
# 1035.2 <= TFA -> the REQ-NC-PLP-003 oracle. (1Gbps links, 1500B/100Mbps flows.)
bounds(Network([srv(GBPS,5e-6),srv(GBPS,10000e-6),srv(GBPS,5e-6)],
               [Flow([TokenBucket(1500*B,MBPS100)],[0,2]),
                Flow([TokenBucket(1500*B,MBPS100)],[1,2])]),
       "converge_bridge  (dataA[0,2] dataB[1,2], sink=2; PLP-003 oracle)")
