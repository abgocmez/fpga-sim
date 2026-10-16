// Written for the ZynqSim universality corpus (not third-party code).
module leaf; initial $display("leaf"); endmodule
module top;
  leaf u();
  initial begin #1 $display("CORPUS_PASS probe-config-block"); $finish; end
endmodule
config cfg;
  design work.top;
  default liblist work;
  instance top.u use work.leaf;
endconfig
