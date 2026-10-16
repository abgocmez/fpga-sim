// Written for the ZynqSim universality corpus (not third-party code).
module top;
  reg r;
  always @* begin
    if (flag) r = 1'b1;
    else      r = 1'b0;
  end
  wire flag = 1'b1;
  initial begin
    #1;
    if (r) $display("CORPUS_PASS use_before_declaration");
    $finish;
  end
endmodule
