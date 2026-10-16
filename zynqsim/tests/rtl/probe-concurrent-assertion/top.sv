// Written for the ZynqSim universality corpus (not third-party code).
module top;
  reg clk = 0, req = 0, ack = 0;
  integer failures = 0;
  always #5 clk = ~clk;
  property p_ack; @(posedge clk) req |-> ##[1:3] ack; endproperty
  a_ack: assert property (p_ack) else failures = failures + 1;
  initial begin
    #12 req = 1; #10 req = 0; ack = 1; #10 ack = 0;
    #50;
    if (failures == 0) $display("CORPUS_PASS probe-concurrent-assertion");
    else $display("CORPUS_FAIL probe-concurrent-assertion");
    $finish;
  end
endmodule
