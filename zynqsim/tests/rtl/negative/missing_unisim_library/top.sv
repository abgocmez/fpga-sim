// Written for the ZynqSim universality corpus (not third-party code).
module top;
  wire q;
  reg c = 0, d = 1;
  FDRE #(.INIT(1'b0)) ff (.C(c), .CE(1'b1), .D(d), .R(1'b0), .Q(q));
  initial begin #10 $display("CORPUS_PASS missing_unisim_library"); $finish; end
endmodule
