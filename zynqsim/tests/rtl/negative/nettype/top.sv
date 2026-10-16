// Written for the ZynqSim universality corpus (not third-party code).
package p;
  typedef struct { real v; } rv_t;
  nettype rv_t wire_rv;
endpackage
module top;
  import p::*;
  wire_rv n;
  initial begin #1 $display("CORPUS_PASS nettype"); $finish; end
endmodule
