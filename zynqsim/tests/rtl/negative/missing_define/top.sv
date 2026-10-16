// Written for the ZynqSim universality corpus (not third-party code).
module top;
  localparam W = `DATA_WIDTH;
  reg [W-1:0] r;
  initial begin r = 1; $display("CORPUS_PASS missing_define"); $finish; end
endmodule
