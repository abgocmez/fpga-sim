// Written for the ZynqSim universality corpus (not third-party code).
interface bus_if(input clk);
  logic [7:0] data;
  logic valid;
  modport master(output data, output valid);
  modport slave(input data, input valid);
endinterface
class drv;
  virtual bus_if.master vif;
  function new(virtual bus_if.master v); vif = v; endfunction
  task send(input [7:0] d); vif.data = d; vif.valid = 1; endtask
endclass
module top;
  reg clk = 0;
  bus_if b(clk);
  drv d;
  initial begin
    d = new(b.master);
    d.send(8'h5A);
    #1 if (b.data == 8'h5A && b.valid) $display("CORPUS_PASS probe-virtual-modport");
    else $display("CORPUS_FAIL probe-virtual-modport");
    $finish;
  end
endmodule
