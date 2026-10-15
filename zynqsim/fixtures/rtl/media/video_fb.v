`timescale 1ns / 1ps
// video_fb: frame-buffer scan-out to an HDMI-style pixel bus, and frame capture
// from one, behind AXI4-Lite with one AXI4 master into DDR.
//
//   0x00 CTRL       bit0 SCAN_EN  drive the pixel bus from the frame buffer
//                   bit1 CAP_EN   capture the next whole incoming frame (self-clearing)
//   0x04 FB_ADDR    frame buffer base (64-byte aligned); 32-bit xRGB per pixel,
//                   one line = H_ACT * 4 bytes
//   0x08 CAP_ADDR   capture buffer base (64-byte aligned), same layout
//   0x0C STATUS     bit0 CAP_DONE (write 1 to clear), bit1 CAP_BUSY
//   0x10 SCAN_FRAMES RO  frames scanned out since SCAN_EN
//   0x14 CAP_FRAMES  RO  frames captured
//   0x18 SIZE       RO  {V_ACT[15:0], H_ACT[15:0]}
//
// Timing is a small VGA-style raster, 25 MHz pixel clock derived from `aclk`
// (100 MHz) by 4, so the whole design is one clock domain. Lines are fetched
// into line buffers in 16-beat INCR bursts that never cross 4 KiB (lines are
// multiples of 64 bytes and bases are 64-byte aligned).
module video_fb #(
    parameter H_ACT = 320, H_FP = 8, H_SYNC = 16, H_BP = 24,
    parameter V_ACT = 240, V_FP = 2, V_SYNC = 2,  V_BP = 10
) (
    input  wire        aclk,
    input  wire        aresetn,
    // AXI4-Lite slave
    input  wire [5:0]  s_awaddr,
    input  wire        s_awvalid,
    output wire        s_awready,
    input  wire [31:0] s_wdata,
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
    // HDMI TX pixel bus (hs/vs active low)
    output wire        pclk_o,
    output reg         hs_o,
    output reg         vs_o,
    output reg         de_o,
    output reg  [23:0] rgb_o,
    // HDMI RX pixel bus (pclk_i is aclk/4 and phase-aligned: sampled with aclk)
    input  wire        pclk_i,
    input  wire        hs_i,
    input  wire        vs_i,
    input  wire        de_i,
    input  wire [23:0] rgb_i
);
    localparam H_TOT = H_ACT + H_FP + H_SYNC + H_BP;
    localparam V_TOT = V_ACT + V_FP + V_SYNC + V_BP;
    localparam BURSTS = H_ACT / 16;                  // 16-beat bursts per line
    localparam LINE_BYTES = H_ACT * 4;

    // ---------------- AXI4-Lite slave ----------------
    reg        scan_en, cap_arm, cap_done;
    reg [31:0] fb_addr, cap_addr;
    reg [31:0] scan_frames, cap_frames;
    reg        cap_active;
    reg        bvalid_r, rvalid_r;
    reg [31:0] rdata_r;

    wire wr_en = s_awvalid & s_wvalid & ~bvalid_r;
    wire rd_en = s_arvalid & ~rvalid_r;
    assign s_awready = wr_en;
    assign s_wready  = wr_en;
    assign s_bresp   = 2'b00;
    assign s_bvalid  = bvalid_r;
    assign s_arready = rd_en;
    assign s_rdata   = rdata_r;
    assign s_rresp   = 2'b00;
    assign s_rvalid  = rvalid_r;

    reg cap_done_set, cap_arm_clr;
    always @(posedge aclk) begin
        if (!aresetn) begin
            scan_en <= 0; cap_arm <= 0; fb_addr <= 0; cap_addr <= 0; bvalid_r <= 0; cap_done <= 0;
        end else begin
            if (cap_done_set) cap_done <= 1'b1;
            if (cap_arm_clr)  cap_arm  <= 1'b0;
            if (wr_en) begin
                bvalid_r <= 1'b1;
                case (s_awaddr[5:2])
                    4'd0: begin scan_en <= s_wdata[0]; if (s_wdata[1]) cap_arm <= 1'b1; end
                    4'd1: fb_addr  <= s_wdata;
                    4'd2: cap_addr <= s_wdata;
                    4'd3: if (s_wdata[0]) cap_done <= 1'b0;
                    default: ;
                endcase
            end else if (bvalid_r && s_bready) begin
                bvalid_r <= 1'b0;
            end
        end
    end

    always @(posedge aclk) begin
        if (!aresetn) begin
            rvalid_r <= 0; rdata_r <= 0;
        end else if (rd_en) begin
            rvalid_r <= 1'b1;
            case (s_araddr[5:2])
                4'd0:    rdata_r <= {30'd0, cap_arm, scan_en};
                4'd1:    rdata_r <= fb_addr;
                4'd2:    rdata_r <= cap_addr;
                4'd3:    rdata_r <= {30'd0, cap_active | cap_arm, cap_done};
                4'd4:    rdata_r <= scan_frames;
                4'd5:    rdata_r <= cap_frames;
                4'd6:    rdata_r <= {V_ACT[15:0], H_ACT[15:0]};
                default: rdata_r <= 0;
            endcase
        end else if (rvalid_r && s_rready) begin
            rvalid_r <= 1'b0;
        end
    end

    // ---------------- line buffers ----------------
    // Two lines each; the second line starts at index 512 (H_ACT must be <= 512).
    reg [23:0] lb [0:1023];           // scan-out, double buffered
    reg [23:0] cb [0:1023];           // capture, double buffered
    reg        f_done, c_done;

    // ---------------- raster timing / scan-out (one pixel per 4 aclk) ----------------
    reg [1:0]  ce_cnt;
    reg [15:0] h, v;
    reg        f_req;
    reg [15:0] f_y;
    wire       ce = (ce_cnt == 2'd3);
    assign pclk_o = scan_en ? ce_cnt[1] : 1'b0;

    wire [15:0] v_next = (v == V_TOT - 1) ? 16'd0 : v + 16'd1;

    reg        started, warm_req;

    always @(posedge aclk) begin
        if (!aresetn || !scan_en) begin
            ce_cnt <= 0; h <= 0; v <= 0; f_req <= 0; f_y <= 0;
            started <= 1'b0; warm_req <= 1'b0;
            hs_o <= 1'b1; vs_o <= 1'b1; de_o <= 1'b0; rgb_o <= 24'd0;
            if (!aresetn) scan_frames <= 0;
        end else if (!started) begin
            // Fetch line 0 before the raster starts so the first frame is complete.
            if (!warm_req) begin
                f_req <= 1'b1; f_y <= 16'd0; warm_req <= 1'b1;
            end else if (f_done) begin
                f_req <= 1'b0; started <= 1'b1;
            end
        end else begin
            ce_cnt <= ce_cnt + 2'd1;
            if (f_done) f_req <= 1'b0;
            if (ce) begin
                de_o <= (h < H_ACT) && (v < V_ACT);
                hs_o <= ~((h >= H_ACT + H_FP) && (h < H_ACT + H_FP + H_SYNC));
                vs_o <= ~((v >= V_ACT + V_FP) && (v < V_ACT + V_FP + V_SYNC));
                rgb_o <= (h < H_ACT && v < V_ACT) ? lb[{v[0], 9'd0} + h[8:0]] : 24'd0;
                if (h == 0 && v_next < V_ACT) begin
                    f_req <= 1'b1;
                    f_y   <= v_next;
                end
                if (h == H_TOT - 1) begin
                    h <= 0;
                    v <= v_next;
                    if (v == V_TOT - 1) scan_frames <= scan_frames + 32'd1;
                end else begin
                    h <= h + 16'd1;
                end
            end
        end
    end

    // ---------------- capture (samples the RX pixel bus on aclk) ----------------
    reg        pclk_d, vs_prev;
    reg [15:0] cap_x, cap_y;
    reg        c_req;
    reg [15:0] c_y;
    wire       pix_ce = pclk_i & ~pclk_d;
    wire       vs_a   = ~vs_i;                       // active-high sync

    always @(posedge aclk) begin
        cap_done_set <= 1'b0;
        cap_arm_clr  <= 1'b0;
        if (!aresetn) begin
            pclk_d <= 0; vs_prev <= 0; cap_x <= 0; cap_y <= 0; c_req <= 0; c_y <= 0;
            cap_active <= 0; cap_frames <= 0;
        end else begin
            pclk_d <= pclk_i;
            if (c_done) c_req <= 1'b0;
            if (pix_ce) begin
                vs_prev <= vs_a;
                if (vs_a && !vs_prev) begin
                    if (cap_active) begin
                        cap_active   <= 1'b0;
                        cap_done_set <= 1'b1;
                        cap_frames   <= cap_frames + 32'd1;
                    end else if (cap_arm) begin
                        cap_active  <= 1'b1;
                        cap_arm_clr <= 1'b1;
                        cap_y <= 0;
                        cap_x <= 0;
                    end
                end
                if (cap_active) begin
                    if (de_i) begin
                        cb[{cap_y[0], 9'd0} + cap_x[8:0]] <= rgb_i;
                        cap_x <= cap_x + 16'd1;
                    end else if (cap_x != 0) begin
                        // line complete: hand it to the write engine
                        c_req <= 1'b1;
                        c_y   <= cap_y;
                        cap_y <= cap_y + 16'd1;
                        cap_x <= 0;
                    end
                end
            end
        end
    end

    // ---------------- AXI4 master: line fetches and capture writes ----------------
    localparam S_IDLE = 3'd0, S_AR = 3'd1, S_R = 3'd2, S_AW = 3'd3, S_W = 3'd4, S_B = 3'd5;
    reg [2:0]  state;
    reg        job_wr;
    reg [15:0] job_y;
    reg [7:0]  burst;
    reg [4:0]  idx;

    wire [31:0] rd_addr = fb_addr  + {16'd0, job_y} * LINE_BYTES + {24'd0, burst} * 64;
    wire [31:0] wr_addr = cap_addr + {16'd0, job_y} * LINE_BYTES + {24'd0, burst} * 64;
    wire [9:0]  cb_idx  = {job_y[0], 9'd0} + {1'b0, burst[4:0], 4'd0} + {5'd0, idx};

    assign m_araddr  = rd_addr;
    assign m_arlen   = 8'd15;
    assign m_arsize  = 3'd2;
    assign m_arburst = 2'b01;
    assign m_arvalid = (state == S_AR);
    assign m_rready  = (state == S_R);
    assign m_awaddr  = wr_addr;
    assign m_awlen   = 8'd15;
    assign m_awsize  = 3'd2;
    assign m_awburst = 2'b01;
    assign m_awvalid = (state == S_AW);
    assign m_wdata   = {8'h00, cb[cb_idx]};
    assign m_wstrb   = 4'hF;
    assign m_wlast   = (idx == 5'd15);
    assign m_wvalid  = (state == S_W);
    assign m_bready  = (state == S_B);

    always @(posedge aclk) begin
        f_done <= 1'b0;
        c_done <= 1'b0;
        if (!aresetn) begin
            state <= S_IDLE; job_wr <= 0; job_y <= 0; burst <= 0; idx <= 0;
        end else begin
            case (state)
            S_IDLE: begin
                burst <= 0;
                if (c_req && !c_done) begin
                    job_wr <= 1'b1; job_y <= c_y; state <= S_AW;
                end else if (f_req && !f_done) begin
                    job_wr <= 1'b0; job_y <= f_y; state <= S_AR;
                end
            end
            S_AR: if (m_arready) begin idx <= 0; state <= S_R; end
            S_R: if (m_rvalid) begin
                lb[{job_y[0], 9'd0} + {burst[4:0], 4'd0} + {4'd0, idx}] <= m_rdata[23:0];
                idx <= idx + 5'd1;
                if (m_rlast || idx == 5'd15) begin
                    if (burst == BURSTS - 1) begin
                        f_done <= 1'b1; state <= S_IDLE;
                    end else begin
                        burst <= burst + 8'd1; state <= S_AR;
                    end
                end
            end
            S_AW: if (m_awready) begin idx <= 0; state <= S_W; end
            S_W: if (m_wready) begin
                idx <= idx + 5'd1;
                if (m_wlast) state <= S_B;
            end
            S_B: if (m_bvalid) begin
                if (burst == BURSTS - 1) begin
                    c_done <= 1'b1; state <= S_IDLE;
                end else begin
                    burst <= burst + 8'd1; state <= S_AW;
                end
            end
            default: state <= S_IDLE;
            endcase
        end
    end
endmodule
