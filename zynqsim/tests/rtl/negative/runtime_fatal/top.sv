// Written for the ZynqSim universality corpus (not third-party code).
module top;
  initial begin
    #5 $fatal(1, "deliberate failure");
  end
endmodule
