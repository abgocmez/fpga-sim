`timescale 1ns / 1ps
// dma_xform: AXI4-Lite controlled memory-to-memory engine.
//   dst[i] = src[i] * SCALE + OFFSET   (32-bit, truncated), CHECKSUM = sum(dst[i])
// Reads/writes DDR through an AXI4 master (INCR bursts of up to 16 beats that never
// cross a 4 KB boundary) and raises `irq` when finished (level, = DONE & IRQ_EN).
//
// Register map (AXI4-Lite, 32-bit):
//   0x00 CTRL      bit0 START (write 1, self-clearing), bit1 IRQ_EN (RW),
//                  bit2 SAT: clamp each result to 0..255 (signed arithmetic)
//   0x04 STATUS    bit0 BUSY (RO), bit1 DONE (W1C), bit2 ERROR (W1C)
//   0x08 SRC       source byte address (word aligned)
//   0x0C DST       destination byte address (word aligned)
//   0x10 LEN       number of 32-bit words (1..65535)
//   0x14 SCALE
//   0x18 OFFSET
//   0x1C CHECKSUM  sum of all words written (RO)
//   0x20 WORDS     words completed so far (RO)
module dma_xform (
    input  wire        aclk,
    input  wire        aresetn,

    // AXI4-Lite slave
    input  wire [5:0]  s_awaddr,
    input  wire        s_awvalid,
    output wire        s_awready,
    input  wire [31:0] s_wdata,
    input  wire [3:0]  s_wstrb,
    input  wire        s_wvalid,
    output wire        s_wready,
    output wire [1:0]  s_bresp,
    output wire        s_bvalid,
    input  wire        s_bready,
    input  wire [5:0]  s_araddr,
    input  wire        s_arvalid,
    output wire        s_arready,
    output wire [31:0] s_rdata,
    output wire [1:0]  s_rresp,
    output wire        s_rvalid,
    input  wire        s_rready,

    // AXI4 master
    output wire [31:0] m_araddr,
    output wire [7:0]  m_arlen,
    output wire [2:0]  m_arsize,
    output wire [1:0]  m_arburst,
    output wire        m_arvalid,
    input  wire        m_arready,
    input  wire [31:0] m_rdata,
    input  wire [1:0]  m_rresp,
    input  wire        m_rlast,
    input  wire        m_rvalid,
    output wire        m_rready,
    output wire [31:0] m_awaddr,
    output wire [7:0]  m_awlen,
    output wire [2:0]  m_awsize,
    output wire [1:0]  m_awburst,
    output wire        m_awvalid,
    input  wire        m_awready,
    output wire [31:0] m_wdata,
    output wire [3:0]  m_wstrb,
    output wire        m_wlast,
    output wire        m_wvalid,
    input  wire        m_wready,
    input  wire [1:0]  m_bresp,
    input  wire        m_bvalid,
    output wire        m_bready,

    output wire        irq
);

    // ---------------------------------------------------------------- registers
    reg        irq_en, sat_en;
    reg [31:0] src_r, dst_r, len_r, scale_r, offset_r;
    reg [31:0] checksum, words_done;
    reg        busy, done, err;
    reg        start_pulse;

    // ----------------------------------------------------------- AXI4-Lite slave
    reg        bvalid_r, rvalid_r;
    reg [31:0] rdata_r;
    wire       wr_en = s_awvalid & s_wvalid & ~bvalid_r;
    wire       rd_en = s_arvalid & ~rvalid_r;

    assign s_awready = wr_en;
    assign s_wready  = wr_en;
    assign s_bresp   = 2'b00;
    assign s_bvalid  = bvalid_r;
    assign s_arready = rd_en;
    assign s_rdata   = rdata_r;
    assign s_rresp   = 2'b00;
    assign s_rvalid  = rvalid_r;

    wire [3:0] widx = s_awaddr[5:2];
    wire [3:0] ridx = s_araddr[5:2];

    // Set by the engine, cleared by software: kept separate to avoid multiple drivers.
    reg        eng_done_set, eng_err_set;
    reg        sw_done_clr, sw_err_clr;

    always @(posedge aclk) begin
        if (!aresetn) begin
            irq_en <= 1'b0; sat_en <= 1'b0; src_r <= 0; dst_r <= 0; len_r <= 0;
            scale_r <= 32'd1; offset_r <= 0;
            bvalid_r <= 1'b0; start_pulse <= 1'b0;
            sw_done_clr <= 1'b0; sw_err_clr <= 1'b0;
        end else begin
            start_pulse <= 1'b0;
            sw_done_clr <= 1'b0;
            sw_err_clr  <= 1'b0;
            if (wr_en) begin
                bvalid_r <= 1'b1;
                case (widx)
                    4'd0: begin
                        irq_en <= s_wdata[1];
                        sat_en <= s_wdata[2];
                        if (s_wdata[0]) start_pulse <= 1'b1;
                    end
                    4'd1: begin
                        if (s_wdata[1]) sw_done_clr <= 1'b1;
                        if (s_wdata[2]) sw_err_clr  <= 1'b1;
                    end
                    4'd2: src_r    <= s_wdata;
                    4'd3: dst_r    <= s_wdata;
                    4'd4: len_r    <= s_wdata;
                    4'd5: scale_r  <= s_wdata;
                    4'd6: offset_r <= s_wdata;
                    default: ;
                endcase
            end else if (bvalid_r && s_bready) begin
                bvalid_r <= 1'b0;
            end
        end
    end

    always @(posedge aclk) begin
        if (!aresetn) begin
            rvalid_r <= 1'b0; rdata_r <= 0;
        end else if (rd_en) begin
            rvalid_r <= 1'b1;
            case (ridx)
                4'd0:    rdata_r <= {29'd0, sat_en, irq_en, 1'b0};
                4'd1:    rdata_r <= {29'd0, err, done, busy};
                4'd2:    rdata_r <= src_r;
                4'd3:    rdata_r <= dst_r;
                4'd4:    rdata_r <= len_r;
                4'd5:    rdata_r <= scale_r;
                4'd6:    rdata_r <= offset_r;
                4'd7:    rdata_r <= checksum;
                4'd8:    rdata_r <= words_done;
                default: rdata_r <= 32'd0;
            endcase
        end else if (rvalid_r && s_rready) begin
            rvalid_r <= 1'b0;
        end
    end

    // ------------------------------------------------------------------ engine
    localparam [2:0] S_IDLE = 3'd0, S_AR = 3'd1, S_R = 3'd2, S_AW = 3'd3,
                     S_W = 3'd4, S_B = 3'd5, S_NEXT = 3'd6;

    reg [2:0]  state;
    reg [31:0] rem, cur_src, cur_dst;
    reg [4:0]  n;                 // words in the current burst (1..16)
    reg [4:0]  idx;
    reg [31:0] buf_mem [0:15];

    // burst length: min(16, rem, words to 4 KB boundary of src and dst)
    wire [12:0] lim_src = (13'd4096 - {1'b0, cur_src[11:0]}) >> 2;
    wire [12:0] lim_dst = (13'd4096 - {1'b0, cur_dst[11:0]}) >> 2;
    reg  [31:0] n_next;
    always @* begin
        n_next = 32'd16;
        if (rem < n_next)               n_next = rem;
        if ({19'd0, lim_src} < n_next)  n_next = {19'd0, lim_src};
        if ({19'd0, lim_dst} < n_next)  n_next = {19'd0, lim_dst};
    end

    wire [31:0] raw_x = buf_mem[idx[3:0]] * scale_r + offset_r;
    wire [31:0] xform = !sat_en ? raw_x : raw_x[31] ? 32'd0 : (raw_x > 32'd255) ? 32'd255 : raw_x;

    assign m_araddr  = cur_src;
    assign m_arlen   = {3'd0, n} - 8'd1;
    assign m_arsize  = 3'd2;
    assign m_arburst = 2'b01;
    assign m_arvalid = (state == S_AR);
    assign m_rready  = (state == S_R);

    assign m_awaddr  = cur_dst;
    assign m_awlen   = {3'd0, n} - 8'd1;
    assign m_awsize  = 3'd2;
    assign m_awburst = 2'b01;
    assign m_awvalid = (state == S_AW);
    assign m_wdata   = xform;
    assign m_wstrb   = 4'hF;
    assign m_wlast   = (idx == n - 5'd1);
    assign m_wvalid  = (state == S_W);
    assign m_bready  = (state == S_B);

    assign irq = done & irq_en;

    always @(posedge aclk) begin
        eng_done_set <= 1'b0;
        eng_err_set  <= 1'b0;
        if (!aresetn) begin
            state <= S_IDLE; busy <= 1'b0; done <= 1'b0; err <= 1'b0;
            checksum <= 0; words_done <= 0; rem <= 0; cur_src <= 0; cur_dst <= 0;
            n <= 0; idx <= 0;
        end else begin
            if (sw_done_clr) done <= 1'b0;
            if (sw_err_clr)  err  <= 1'b0;

            case (state)
            S_IDLE: begin
                if (start_pulse) begin
                    if (len_r == 0 || len_r > 32'd65535 ||
                        src_r[1:0] != 2'b00 || dst_r[1:0] != 2'b00) begin
                        err <= 1'b1; done <= 1'b1;        // rejected
                    end else begin
                        busy <= 1'b1; done <= 1'b0; err <= 1'b0;
                        checksum <= 0; words_done <= 0;
                        rem <= len_r; cur_src <= src_r; cur_dst <= dst_r;
                        state <= S_NEXT;
                    end
                end
            end
            S_NEXT: begin
                if (rem == 0 || err) begin
                    busy <= 1'b0; done <= 1'b1; state <= S_IDLE;
                end else begin
                    n <= n_next[4:0];
                    state <= S_AR;
                end
            end
            S_AR: if (m_arready) begin idx <= 0; state <= S_R; end
            S_R: if (m_rvalid) begin
                buf_mem[idx[3:0]] <= m_rdata;
                if (m_rresp != 2'b00) err <= 1'b1;
                idx <= idx + 5'd1;
                if (m_rlast || idx == n - 5'd1) state <= S_AW;
            end
            S_AW: if (m_awready) begin idx <= 0; state <= S_W; end
            S_W: if (m_wready) begin
                checksum <= checksum + xform;
                idx <= idx + 5'd1;
                if (m_wlast) state <= S_B;
            end
            S_B: if (m_bvalid) begin
                if (m_bresp != 2'b00) err <= 1'b1;
                words_done <= words_done + {27'd0, n};
                rem <= rem - {27'd0, n};
                cur_src <= cur_src + {25'd0, n, 2'b00};
                cur_dst <= cur_dst + {25'd0, n, 2'b00};
                state <= S_NEXT;
            end
            default: state <= S_IDLE;
            endcase
        end
    end

endmodule
