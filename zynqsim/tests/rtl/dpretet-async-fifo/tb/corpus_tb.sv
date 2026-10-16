// Self-checking testbench written for the ZynqSim universality corpus (not upstream code).
// A dual-clock FIFO crossing from a 100 MHz write clock to a 71 MHz read clock. 500 words are
// written with random gaps and must be read back in order, none lost, with the full / empty
// flags never violated.
`timescale 1ns / 1ps

module corpus_tb;
    reg wclk = 0, rclk = 0;
    reg wrst_n = 0, rrst_n = 0;
    always #5 wclk = ~wclk;
    always #7 rclk = ~rclk;

    reg  winc = 0;
    reg  [7:0] wdata = 0;
    wire wfull, awfull;
    reg  rinc = 0;
    wire [7:0] rdata;
    wire rempty, arempty;

    async_fifo #(.DSIZE(8), .ASIZE(4), .FALLTHROUGH("TRUE")) dut (
        .wclk(wclk), .wrst_n(wrst_n), .winc(winc), .wdata(wdata), .wfull(wfull),
        .awfull(awfull),
        .rclk(rclk), .rrst_n(rrst_n), .rinc(rinc), .rdata(rdata), .rempty(rempty),
        .arempty(arempty));

    localparam N = 500;
    integer sent = 0;
    integer got = 0;
    integer errors = 0;
    reg [31:0] lw = 32'h1357_9BDF;
    reg [31:0] lr = 32'h2468_ACE1;

    function [31:0] step(input [31:0] x);
        reg [31:0] y;
        begin
            y = x ^ (x << 13);
            y = y ^ (y >> 17);
            y = y ^ (y << 5);
            step = y;
        end
    endfunction

    // Write side: a word is held on wdata/winc until the FIFO takes it (winc && !wfull).
    reg accepted;
    always @(posedge wclk) begin
        lw <= step(lw);
        if (!wrst_n) begin
            winc <= 1'b0;
        end else begin
            accepted = winc && !wfull;
            if (accepted) sent = sent + 1;
            if ((!winc || accepted) && sent < N && lw[4] && lw[11]) begin
                wdata <= sent[7:0] * 8'd7 + 8'd3;
                winc  <= 1'b1;
            end else if (accepted) begin
                winc <= 1'b0;
            end
        end
    end

    // Read side: first-word-fall-through, so rdata is the head whenever !rempty.
    always @(posedge rclk) begin
        lr <= step(lr);
        if (!rrst_n) begin
            rinc <= 1'b0;
        end else begin
            rinc <= 1'b0;
            if (!rempty && !rinc && lr[6]) begin
                if (rdata !== (got[7:0] * 8'd7 + 8'd3)) begin
                    $display("word %0d: got %02x expected %02x", got, rdata, got[7:0] * 8'd7 + 8'd3);
                    errors = errors + 1;
                end
                rinc <= 1'b1;
                got = got + 1;
            end
        end
    end

    initial begin
        repeat (6) @(posedge wclk);
        wrst_n <= 1'b1;
        rrst_n <= 1'b1;
        wait (got == N);
        repeat (10) @(posedge rclk);
        if (!rempty) begin
            $display("FIFO not empty after the last word");
            errors = errors + 1;
        end
        if (errors == 0) $display("CORPUS_PASS dpretet-async-fifo");
        else $display("CORPUS_FAIL dpretet-async-fifo (%0d errors)", errors);
        $finish;
    end

    initial begin
        #2000000;
        $display("CORPUS_FAIL dpretet-async-fifo (timeout: sent %0d, read %0d)", sent, got);
        $finish;
    end
endmodule
