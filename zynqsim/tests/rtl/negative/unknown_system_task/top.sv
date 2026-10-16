// Written for the ZynqSim universality corpus (not third-party code).
module top;
  initial begin
    $my_vpi_hello(1, 2);
    $display("CORPUS_PASS unknown_system_task");
    $finish;
  end
endmodule
