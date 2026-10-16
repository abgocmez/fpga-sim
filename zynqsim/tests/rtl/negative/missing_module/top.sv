// Written for the ZynqSim universality corpus (not third-party code).
module top;
  wire y;
  vendor_black_box u0 (.y(y));
  initial begin #1 $display("CORPUS_PASS missing_module"); $finish; end
endmodule
