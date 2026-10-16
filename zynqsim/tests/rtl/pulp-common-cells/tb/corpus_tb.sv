// Self-checking testbench written for the ZynqSim universality corpus (not upstream code).
// Exercises five PULP common_cells blocks (SystemVerilog: packages, `parameter type`,
// `always_comb`, `genvar`, macro headers with assertions): popcount, leading/trailing zero
// counters, binary<->gray converters, the FIFO against a queue model, and the round-robin
// arbiter tree (fair share among four always-requesting inputs).
`timescale 1ns / 1ps

module corpus_tb;
    import cc_pkg::*;

    logic clk = 1'b0;
    logic rst_n = 1'b0;
    always #5 clk = ~clk;

    int errors = 0;

    // ---- popcount, lzc, gray ------------------------------------------------------------
    logic [7:0] pc_in;
    logic [3:0] pc_out;
    cc_popcount #(.InputWidth(8)) u_pop (.data_i(pc_in), .popcount_o(pc_out));

    logic [15:0] lz_in;
    logic [3:0]  lz_lead_cnt, lz_trail_cnt;
    logic        lz_lead_empty, lz_trail_empty;
    cc_lzc #(.Width(16), .Mode(LZC_LEADING_ZERO_CNT)) u_lead (.in_i(lz_in), .cnt_o(lz_lead_cnt), .empty_o(lz_lead_empty));
    cc_lzc #(.Width(16), .Mode(LZC_TRAILING_ZERO_CNT)) u_trail (.in_i(lz_in), .cnt_o(lz_trail_cnt), .empty_o(lz_trail_empty));

    logic [7:0] bin, gray, back;
    cc_binary_to_gray #(.Width(8)) u_b2g (.a_i(bin), .z_o(gray));
    cc_gray_to_binary #(.Width(8)) u_g2b (.a_i(gray), .z_o(back));

    task automatic check_comb();
        int expect_pop;
        int lead, trail;
        for (int v = 0; v < 256; v++) begin
            pc_in = v[7:0];
            bin = v[7:0];
            #1;
            expect_pop = 0;
            for (int b = 0; b < 8; b++) expect_pop += v[b];
            if (pc_out !== expect_pop[3:0]) begin
                $display("popcount(%0h) = %0d, expected %0d", v, pc_out, expect_pop);
                errors++;
            end
            if (back !== bin) begin
                $display("gray round trip of %0h gave %0h", bin, back);
                errors++;
            end
        end
        for (int s = 0; s < 16; s++) begin
            lz_in = 16'h1 << s;
            #1;
            lead = 15 - s;
            trail = s;
            if (lz_lead_cnt !== lead[3:0] || lz_lead_empty) begin
                $display("leading zeros of 1<<%0d: %0d", s, lz_lead_cnt);
                errors++;
            end
            if (lz_trail_cnt !== trail[3:0] || lz_trail_empty) begin
                $display("trailing zeros of 1<<%0d: %0d", s, lz_trail_cnt);
                errors++;
            end
        end
        lz_in = 16'h0;
        #1;
        if (!lz_lead_empty || !lz_trail_empty) begin
            $display("lzc empty flag missing");
            errors++;
        end
    endtask

    // ---- FIFO ------------------------------------------------------------------------------
    logic       fifo_full, fifo_empty;
    logic [2:0] fifo_usage;
    logic [7:0] fifo_din, fifo_dout;
    logic       fifo_push = 1'b0, fifo_pop = 1'b0;
    cc_fifo #(.FallThrough(1'b0), .DataWidth(8), .Depth(4)) u_fifo (
        .clk_i(clk), .rst_ni(rst_n), .clr_i(1'b0), .flush_i(1'b0),
        .full_o(fifo_full), .empty_o(fifo_empty), .usage_o(fifo_usage),
        .data_i(fifo_din), .push_i(fifo_push), .data_o(fifo_dout), .pop_i(fifo_pop));

    byte unsigned model[$];
    int pushed = 0, popped = 0;
    int unsigned seed = 32'h1234_5678;

    task automatic fifo_run();
        for (int cyc = 0; cyc < 300; cyc++) begin
            @(negedge clk);
            seed = seed * 32'd1664525 + 32'd1013904223;
            fifo_push = 1'b0;
            fifo_pop = 1'b0;
            if (seed[20] && !fifo_full && pushed < 100) begin
                fifo_din = pushed[7:0] ^ 8'hA5;
                fifo_push = 1'b1;
                model.push_back(fifo_din);
                pushed++;
            end
            if (seed[22] && !fifo_empty) begin
                fifo_pop = 1'b1;
                if (fifo_dout !== model[0]) begin
                    $display("fifo word %0d: got %02x expected %02x", popped, fifo_dout, model[0]);
                    errors++;
                end
                void'(model.pop_front());
                popped++;
            end
        end
        @(negedge clk);
        fifo_push = 1'b0;
        fifo_pop = 1'b0;
        // Drain.
        while (!fifo_empty) begin
            fifo_pop = 1'b1;
            if (fifo_dout !== model[0]) begin
                $display("drain word %0d: got %02x expected %02x", popped, fifo_dout, model[0]);
                errors++;
            end
            void'(model.pop_front());
            popped++;
            @(negedge clk);
        end
        fifo_pop = 1'b0;
        if (popped !== pushed) begin
            $display("fifo lost words: pushed %0d popped %0d", pushed, popped);
            errors++;
        end
    endtask

    // ---- round-robin arbiter ----------------------------------------------------------------
    logic [3:0] arb_req = 4'hF;
    logic [3:0] arb_gnt;
    logic       arb_req_o;
    logic [7:0] arb_data_o;
    logic [1:0] arb_idx;
    logic [3:0][7:0] arb_data;
    assign arb_data = {8'd40, 8'd30, 8'd20, 8'd10};
    cc_rr_arb_tree #(.NumIn(4), .DataWidth(8), .AxiVldRdy(1'b0), .LockIn(1'b0)) u_arb (
        .clk_i(clk), .rst_ni(rst_n), .clr_i(1'b0), .rr_i(2'd0),
        .req_i(arb_req), .gnt_o(arb_gnt), .data_i(arb_data),
        .req_o(arb_req_o), .gnt_i(1'b1), .data_o(arb_data_o), .idx_o(arb_idx));

    int share [4];

    task automatic arb_run();
        for (int i = 0; i < 4; i++) share[i] = 0;
        for (int cyc = 0; cyc < 40; cyc++) begin
            @(negedge clk);
            if (arb_req_o) begin
                share[arb_idx]++;
                if (arb_data_o !== (8'd10 * (arb_idx + 1))) begin
                    $display("arbiter data %0d for input %0d", arb_data_o, arb_idx);
                    errors++;
                end
            end
        end
        for (int i = 0; i < 4; i++)
            if (share[i] !== 10) begin
                $display("arbiter share of input %0d = %0d, expected 10", i, share[i]);
                errors++;
            end
    endtask

    initial begin
        check_comb();
        repeat (3) @(posedge clk);
        rst_n = 1'b1;
        @(negedge clk);
        fork
            fifo_run();
            arb_run();
        join
        if (errors == 0) $display("CORPUS_PASS pulp-common-cells");
        else $display("CORPUS_FAIL pulp-common-cells (%0d errors)", errors);
        $finish;
    end

    initial begin
        #200000;
        $display("CORPUS_FAIL pulp-common-cells (timeout)");
        $finish;
    end
endmodule
