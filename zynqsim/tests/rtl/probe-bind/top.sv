// Written for the ZynqSim universality corpus (not third-party code).
module dut(input clk, output reg q);
  initial q = 0;
  always @(posedge clk) q <= ~q;
endmodule
module chk(input clk, input q);
  integer seen = 0;
  always @(posedge clk) seen <= seen + 1;
endmodule
bind dut chk u_chk(.clk(clk), .q(q));
module top;
  reg clk = 0;
  wire q;
  dut d(clk, q);
  always #5 clk = ~clk;
  initial begin
    #30;
    if (d.u_chk.seen == 3) $display("CORPUS_PASS probe-bind");
    else $display("CORPUS_FAIL probe-bind (the bound checker saw %0d edges)", d.u_chk.seen);
    $finish;
  end
endmodule
