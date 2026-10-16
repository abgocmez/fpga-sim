`timescale 1ns / 1ps
// zs_axi_checker: AXI protocol checker for zynqsim testbenches (ADR-025).
//
// It only observes: it drives nothing, so it cannot change what the design sees.
// Every finding goes to the host through the DPI function zs_protocol_check, which the
// host turns into a trace marker (`protocol check <severity> at <t> ns: ...`).
//
//   ID    0 = the AXI4-Lite slave the testbench drives, 1 = the AXI4 master the IP drives
//   FULL  1 = AXI4 (bursts: 4 KB, WLAST/RLAST, WRAP); 0 = AXI4-Lite (VALID/READY,
//         reset and responses only)
//
// Rules (codes shared with crates/pl-api/src/protocol.rs; channel: 0 AW, 1 W, 2 B, 3 AR, 4 R):
//   1 VALID_DROP        VALID fell before READY was seen
//   2 PAYLOAD_UNSTABLE  payload changed while VALID && !READY
//   3 RESET_VALID       VALID high while ARESETN is low (from the second reset cycle)
//   4 CROSS_4KB         INCR burst crosses a 4 KB boundary
//   5 LAST_EARLY        WLAST/RLAST before the last beat
//   6 LAST_MISSING      last beat without WLAST/RLAST
//   7 WRAP_ALIGN        WRAP start address not aligned to the transfer size
//   8 WRAP_LEN          WRAP length not 2, 4, 8 or 16 beats
//   9 SIZE              AxSIZE wider than the data bus
//  10 RESP              SLVERR/DECERR (informational)
//
// Limits: WLAST is checked only while every W beat follows its AW; a W beat that arrives
// before any AW turns the WLAST check off for the rest of the run (AXI allows that order).
// INCR bursts with unaligned start addresses are legal and not reported.
// Verilog-2001 style apart from the DPI import and `longint`; no assertions.
module zs_axi_checker #(
    parameter integer ID   = 0,
    parameter integer FULL = 0,
    parameter integer AW   = 32,
    parameter integer DW   = 32
) (
    input  wire              aclk,
    input  wire              aresetn,
    input  wire              awvalid,
    input  wire              awready,
    input  wire [AW-1:0]     awaddr,
    input  wire [7:0]        awlen,
    input  wire [2:0]        awsize,
    input  wire [1:0]        awburst,
    input  wire              wvalid,
    input  wire              wready,
    input  wire [DW-1:0]     wdata,
    input  wire [DW/8-1:0]   wstrb,
    input  wire              wlast,
    input  wire              bvalid,
    input  wire              bready,
    input  wire [1:0]        bresp,
    input  wire              arvalid,
    input  wire              arready,
    input  wire [AW-1:0]     araddr,
    input  wire [7:0]        arlen,
    input  wire [2:0]        arsize,
    input  wire [1:0]        arburst,
    input  wire              rvalid,
    input  wire              rready,
    input  wire [DW-1:0]     rdata,
    input  wire [1:0]        rresp,
    input  wire              rlast
);
    import "DPI-C" function void zs_protocol_check(input int port, input int channel,
                                                   input int rule, input int info,
                                                   input longint detail, input longint now_ns);

    // One finding: `info` 0 = violation, 1 = informational.
    task note(input integer ch, input integer rule, input integer info, input [63:0] detail);
        begin
            zs_protocol_check(ID, ch, rule, info, detail, $time);
        end
    endtask

    // Payload of each channel, as one vector, to compare cycle to cycle.
    wire [AW+12:0]    aw_pl = {awaddr, awlen, awsize, awburst};
    wire [AW+12:0]    ar_pl = {araddr, arlen, arsize, arburst};
    wire [DW+DW/8:0]  w_pl  = {wdata, wstrb, wlast};
    wire [DW+2:0]     r_pl  = {rdata, rresp, rlast};

    reg [AW+12:0]    aw_hold = 0, ar_hold = 0;
    reg [DW+DW/8:0]  w_hold = 0;
    reg [DW+2:0]     r_hold = 0;
    reg [1:0]        b_hold = 0;
    reg              aw_pend = 1'b0, w_pend = 1'b0, b_pend = 1'b0, ar_pend = 1'b0, r_pend = 1'b0;

    // Rule 1 (VALID held until READY) and 2 (payload stable), every channel, AXI4-Lite too.
    always @(posedge aclk) begin
        if (!aresetn) begin
            aw_pend <= 1'b0; w_pend <= 1'b0; b_pend <= 1'b0; ar_pend <= 1'b0; r_pend <= 1'b0;
        end else begin
            if (aw_pend) begin
                if (awvalid !== 1'b1) note(0, 1, 0, 0);
                else if (aw_pl !== aw_hold) note(0, 2, 0, 0);
            end
            if (w_pend) begin
                if (wvalid !== 1'b1) note(1, 1, 0, 0);
                else if (w_pl !== w_hold) note(1, 2, 0, 0);
            end
            if (b_pend) begin
                if (bvalid !== 1'b1) note(2, 1, 0, 0);
                else if (bresp !== b_hold) note(2, 2, 0, 0);
            end
            if (ar_pend) begin
                if (arvalid !== 1'b1) note(3, 1, 0, 0);
                else if (ar_pl !== ar_hold) note(3, 2, 0, 0);
            end
            if (r_pend) begin
                if (rvalid !== 1'b1) note(4, 1, 0, 0);
                else if (r_pl !== r_hold) note(4, 2, 0, 0);
            end
            aw_pend <= (awvalid === 1'b1) && (awready !== 1'b1);
            w_pend  <= (wvalid === 1'b1) && (wready !== 1'b1);
            b_pend  <= (bvalid === 1'b1) && (bready !== 1'b1);
            ar_pend <= (arvalid === 1'b1) && (arready !== 1'b1);
            r_pend  <= (rvalid === 1'b1) && (rready !== 1'b1);
            aw_hold <= aw_pl;
            w_hold  <= w_pl;
            b_hold  <= bresp;
            ar_hold <= ar_pl;
            r_hold  <= r_pl;
        end
    end

    // Rule 3: after reset every VALID is low. Reported once per channel and reset.
    reg [1:0] rst_seen = 2'd0;
    reg [4:0] rst_rep  = 5'd0;
    wire [4:0] valids = {rvalid, arvalid, bvalid, wvalid, awvalid};
    integer k;
    always @(posedge aclk) begin
        if (!aresetn) begin
            if (rst_seen != 2'd3) rst_seen <= rst_seen + 2'd1;
            if (rst_seen != 2'd0) begin
                for (k = 0; k < 5; k = k + 1) begin
                    if (valids[k] === 1'b1 && !rst_rep[k]) begin
                        note(k, 3, 0, 0);
                        rst_rep[k] <= 1'b1;
                    end
                end
            end
        end else begin
            rst_seen <= 2'd0;
            rst_rep  <= 5'd0;
        end
    end

    // Rules 4..10 and the responses: burst bookkeeping, one block so that the queues have
    // a single writer. Blocking assignments on purpose.
    reg [7:0] awq [0:15];
    reg [7:0] arq [0:15];
    reg [3:0] awq_wr = 0, awq_rd = 0, arq_wr = 0, arq_rd = 0;
    integer   awq_n = 0, arq_n = 0;
    reg [7:0] w_idx = 0, r_idx = 0;
    reg       wlast_off = 1'b0, rlast_off = 1'b0;
    reg [63:0] first, lastb, bytes, span;
    reg       exp_last;

    always @(posedge aclk) begin
        if (!aresetn) begin
            awq_wr = 0; awq_rd = 0; arq_wr = 0; arq_rd = 0;
            awq_n = 0; arq_n = 0; w_idx = 0; r_idx = 0;
            wlast_off = 1'b0; rlast_off = 1'b0;
        end else if (FULL != 0) begin
            // AW handshake: 4 KB, WRAP alignment and length, size; queue the length.
            if (awvalid === 1'b1 && awready === 1'b1) begin
                first = awaddr;
                bytes = 64'd1 << awsize;
                span = (awlen + 64'd1) * bytes;
                lastb = first + span - 64'd1;
                if (awburst == 2'b01 && (first >> 12) != (lastb >> 12)) note(0, 4, 0, first);
                if (awburst == 2'b10) begin
                    if ((first & (bytes - 64'd1)) != 0) note(0, 7, 0, first);
                    if (awlen != 8'd1 && awlen != 8'd3 && awlen != 8'd7 && awlen != 8'd15)
                        note(0, 8, 0, awlen);
                end
                if (bytes > DW / 8) note(0, 9, 0, awsize);
                if (awq_n < 16) begin
                    awq[awq_wr] = awlen;
                    awq_wr = awq_wr + 4'd1;
                    awq_n = awq_n + 1;
                end
            end
            // AR handshake: the same address rules; queue the length.
            if (arvalid === 1'b1 && arready === 1'b1) begin
                first = araddr;
                bytes = 64'd1 << arsize;
                span = (arlen + 64'd1) * bytes;
                lastb = first + span - 64'd1;
                if (arburst == 2'b01 && (first >> 12) != (lastb >> 12)) note(3, 4, 0, first);
                if (arburst == 2'b10) begin
                    if ((first & (bytes - 64'd1)) != 0) note(3, 7, 0, first);
                    if (arlen != 8'd1 && arlen != 8'd3 && arlen != 8'd7 && arlen != 8'd15)
                        note(3, 8, 0, arlen);
                end
                if (bytes > DW / 8) note(3, 9, 0, arsize);
                if (arq_n < 16) begin
                    arq[arq_wr] = arlen;
                    arq_wr = arq_wr + 4'd1;
                    arq_n = arq_n + 1;
                end
            end
            // W beat: WLAST on the last beat of the head burst only.
            if (wvalid === 1'b1 && wready === 1'b1 && !wlast_off) begin
                if (awq_n == 0) begin
                    wlast_off = 1'b1;
                end else begin
                    exp_last = (w_idx == awq[awq_rd]);
                    if (exp_last && wlast !== 1'b1) note(1, 6, 0, w_idx);
                    if (!exp_last && wlast === 1'b1) note(1, 5, 0, w_idx);
                    if (exp_last) begin
                        awq_rd = awq_rd + 4'd1; awq_n = awq_n - 1; w_idx = 0;
                    end else begin
                        w_idx = w_idx + 8'd1;
                    end
                end
            end
            // R beat: RLAST on the last beat of the head read burst only.
            if (rvalid === 1'b1 && rready === 1'b1 && !rlast_off) begin
                if (arq_n == 0) begin
                    rlast_off = 1'b1;
                end else begin
                    exp_last = (r_idx == arq[arq_rd]);
                    if (exp_last && rlast !== 1'b1) note(4, 6, 0, r_idx);
                    if (!exp_last && rlast === 1'b1) note(4, 5, 0, r_idx);
                    if (exp_last) begin
                        arq_rd = arq_rd + 4'd1; arq_n = arq_n - 1; r_idx = 0;
                    end else begin
                        r_idx = r_idx + 8'd1;
                    end
                end
            end
        end
    end

    // Rule 10: responses other than OKAY are reported as information.
    always @(posedge aclk) begin
        if (aresetn) begin
            if (bvalid === 1'b1 && bready === 1'b1 && bresp >= 2'b10) note(2, 10, 1, bresp);
            if (rvalid === 1'b1 && rready === 1'b1 && rresp >= 2'b10) note(4, 10, 1, rresp);
        end
    end
endmodule
