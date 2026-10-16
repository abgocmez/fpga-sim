// Written for the ZynqSim universality corpus (not third-party code).
`include "no_such_header.vh"
module top;
  initial begin $display("CORPUS_PASS missing_include"); $finish; end
endmodule
