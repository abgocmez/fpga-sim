// Written for the ZynqSim universality corpus (not third-party code).
module top;
  reg [3:0] v = 0;
  reg clk = 0;
  covergroup cg @(posedge clk);
    coverpoint v { bins lo = {[0:7]}; bins hi = {[8:15]}; }
  endgroup
  cg c = new();
  initial begin
    repeat (4) begin #1 clk = ~clk; v = v + 5; end
    $display("CORPUS_PASS probe-covergroup");
    $finish;
  end
endmodule
