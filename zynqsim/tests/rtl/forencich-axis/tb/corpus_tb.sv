// Self-checking testbench written for the ZynqSim universality corpus (not upstream code).
// source -> axis_fifo (64 words) -> axis_register (skid buffer) -> sink, with pseudo-random
// stalls on both ends. 200 words in frames of 16 must arrive in order with tlast on the right
// words and nothing dropped.
`timescale 1ns / 1ps

module corpus_tb;
    reg clk = 0;
    reg rst = 1;
    always #5 clk = ~clk;

    reg  [7:0] s_tdata = 8'h00;
    reg        s_tvalid = 1'b0;
    wire       s_tready;
    reg        s_tlast = 1'b0;

    wire [7:0] f_tdata;
    wire       f_tvalid, f_tready, f_tlast;

    wire [7:0] m_tdata;
    wire       m_tvalid, m_tlast;
    reg        m_tready = 1'b0;

    wire [6:0] depth;
    wire       overflow, bad_frame, good_frame;
    wire [6:0] depth_commit;

    axis_fifo #(.DEPTH(64), .DATA_WIDTH(8), .KEEP_ENABLE(0), .LAST_ENABLE(1),
                .USER_ENABLE(0), .FRAME_FIFO(0)) u_fifo (
        .clk(clk), .rst(rst),
        .s_axis_tdata(s_tdata), .s_axis_tkeep(1'b1), .s_axis_tvalid(s_tvalid),
        .s_axis_tready(s_tready), .s_axis_tlast(s_tlast), .s_axis_tid(8'd0),
        .s_axis_tdest(8'd0), .s_axis_tuser(1'b0),
        .m_axis_tdata(f_tdata), .m_axis_tkeep(), .m_axis_tvalid(f_tvalid),
        .m_axis_tready(f_tready), .m_axis_tlast(f_tlast), .m_axis_tid(), .m_axis_tdest(),
        .m_axis_tuser(),
        .pause_req(1'b0), .pause_ack(),
        .status_depth(depth), .status_depth_commit(depth_commit), .status_overflow(overflow),
        .status_bad_frame(bad_frame), .status_good_frame(good_frame));

    axis_register #(.DATA_WIDTH(8), .KEEP_ENABLE(0), .LAST_ENABLE(1), .USER_ENABLE(0),
                    .REG_TYPE(2)) u_reg (
        .clk(clk), .rst(rst),
        .s_axis_tdata(f_tdata), .s_axis_tkeep(1'b1), .s_axis_tvalid(f_tvalid),
        .s_axis_tready(f_tready), .s_axis_tlast(f_tlast), .s_axis_tid(8'd0),
        .s_axis_tdest(8'd0), .s_axis_tuser(1'b0),
        .m_axis_tdata(m_tdata), .m_axis_tkeep(), .m_axis_tvalid(m_tvalid),
        .m_axis_tready(m_tready), .m_axis_tlast(m_tlast), .m_axis_tid(), .m_axis_tdest(),
        .m_axis_tuser());

    localparam N = 200;
    integer sent = 0;
    integer got = 0;
    integer errors = 0;
    reg [31:0] lfsr = 32'hACE1_2468;

    function [31:0] next(input [31:0] x);
        reg [31:0] y;
        begin
            y = x ^ (x << 13);
            y = y ^ (y >> 17);
            y = y ^ (y << 5);
            next = y;
        end
    endfunction

    // Source: a word is offered when the random bit allows, and held until accepted.
    always @(posedge clk) begin
        if (rst) begin
            s_tvalid <= 1'b0;
            s_tlast  <= 1'b0;
        end else begin
            lfsr <= next(lfsr);
            if (s_tvalid && s_tready) begin
                sent = sent + 1;
                s_tvalid <= 1'b0;
            end
            if ((!s_tvalid || s_tready) && sent < N && lfsr[3]) begin
                s_tdata  <= sent[7:0] ^ 8'h5A;
                s_tlast  <= (sent % 16) == 15;
                s_tvalid <= 1'b1;
            end else if (s_tvalid && s_tready) begin
                s_tvalid <= 1'b0;
            end
        end
    end

    // Sink: random back pressure; checks data and tlast.
    always @(posedge clk) begin
        if (rst) m_tready <= 1'b0;
        else begin
            m_tready <= lfsr[9] | lfsr[10];
            if (m_tvalid && m_tready) begin
                if (m_tdata !== ((got[7:0]) ^ 8'h5A)) begin
                    $display("word %0d: got %02x expected %02x", got, m_tdata, got[7:0] ^ 8'h5A);
                    errors = errors + 1;
                end
                if (m_tlast !== ((got % 16) == 15)) begin
                    $display("word %0d: tlast = %b", got, m_tlast);
                    errors = errors + 1;
                end
                got = got + 1;
            end
        end
    end

    initial begin
        repeat (5) @(posedge clk);
        rst <= 1'b0;
        wait (got == N);
        repeat (20) @(posedge clk);
        if (got !== N) errors = errors + 1;
        if (overflow) begin
            $display("fifo overflow flagged");
            errors = errors + 1;
        end
        if (errors == 0) $display("CORPUS_PASS forencich-axis");
        else $display("CORPUS_FAIL forencich-axis (%0d errors)", errors);
        $finish;
    end

    initial begin
        #500000;
        $display("CORPUS_FAIL forencich-axis (timeout: sent %0d, received %0d)", sent, got);
        $finish;
    end
endmodule
