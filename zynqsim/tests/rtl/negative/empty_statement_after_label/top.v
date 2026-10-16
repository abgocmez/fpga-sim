// Written for the ZynqSim universality corpus (not third-party code).
module top;
  task t;
    begin : blk;
      $display("in the task");
    end
  endtask
  initial begin
    t;
    $display("CORPUS_PASS empty_statement_after_label");
    $finish;
  end
endmodule
