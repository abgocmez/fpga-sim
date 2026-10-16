// Written for the ZynqSim universality corpus (not third-party code).
import uvm_pkg::*;
`include "uvm_macros.svh"
class t extends uvm_test;
  `uvm_component_utils(t)
  function new(string name, uvm_component parent); super.new(name, parent); endfunction
  task run_phase(uvm_phase phase);
    phase.raise_objection(this);
    `uvm_info("T", "CORPUS_PASS probe-uvm-with-library", UVM_NONE)
    phase.drop_objection(this);
  endtask
endclass
module top;
  initial begin run_test("t"); end
endmodule
