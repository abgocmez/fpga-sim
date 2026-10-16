// Written for the ZynqSim universality corpus (not third-party code).
module sub(inout interconnect x); endmodule
module top;
  interconnect w;
  sub u(w);
  initial begin #1 $display("CORPUS_PASS generic_interconnect"); $finish; end
endmodule
