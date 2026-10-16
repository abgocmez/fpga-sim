// Written for the ZynqSim universality corpus (not third-party code).
module top;
  import "DPI-C" function int c_add(input int a, input int b);
  initial begin
    $display("c_add=%0d", c_add(1, 2));
    $finish;
  end
endmodule
