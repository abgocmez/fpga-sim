// Written for the ZynqSim universality corpus (not third-party code).
module top;
  reg [7:0] m [0:3];
  initial begin
    $readmemh("no_such_file.hex", m);
    $display("CORPUS_PASS readmem_missing_file");
    $finish;
  end
endmodule
