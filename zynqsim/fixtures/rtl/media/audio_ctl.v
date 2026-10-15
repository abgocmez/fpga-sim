`timescale 1ns / 1ps
// audio_ctl: a small I2S audio block for the Zybo Z7 codec, behind AXI4-Lite.
//
//   0x00 CTRL      bit0 ENABLE   run the I2S master (BCLK/LRCLK/PBDAT)
//                  bit1 TONE_EN  DAC data = triangle tone (else silence)
//                  bit2 LOOP     DAC data = ADC data of the previous frame
//   0x04 STEP      tone phase step per frame; f_hz = STEP * 48000 / 2^32
//   0x08 AMP       tone amplitude, 0..32767
//   0x0C FRAMES    RO  frames sent since ENABLE
//   0x10 ADC_L     RO  last left ADC sample (sign-extended)
//   0x14 ADC_R     RO  last right ADC sample
//   0x18 ADC_PEAK  RO  max |left ADC sample| since last write (any write clears)
//   0x1C I2C       bit0 SCL release, bit1 SDA release (1 = released/high),
//                  bit8 RO: SDA as seen on the pin     (bit-banged master)
//
// Audio logic runs on `mclk` (12.288 MHz): BCLK = MCLK/4 (64 x 48 kHz),
// LRCLK = BCLK/64 (48 kHz). Register values cross from the AXI clock to MCLK
// as quasi-static configuration; this is a simulation demo, not a CDC-clean IP.
module audio_ctl (
    input  wire        aclk,
    input  wire        aresetn,
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

    input  wire        mclk,
    output wire        bclk,
    output wire        lrclk,
    output wire        pbdat,
    input  wire        recdat,
    output wire        scl_o,      // 1 = release (pulled high)
    output wire        sda_o,
    input  wire        sda_i
);
    // ---------------- AXI4-Lite slave ----------------
    reg [2:0]  ctrl;
    reg [31:0] step, amp32, i2c;
    reg        bvalid_r, rvalid_r;
    reg [31:0] rdata_r;
    reg        peak_clr;

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

    // status from the MCLK domain (read through two flops)
    wire [31:0] frames_m, adc_l_m, adc_r_m, peak_m;
    reg  [31:0] frames_a, adc_l_a, adc_r_a, peak_a;
    reg  [31:0] frames_b, adc_l_b, adc_r_b, peak_b;
    always @(posedge aclk) begin
        frames_a <= frames_m; frames_b <= frames_a;
        adc_l_a  <= adc_l_m;  adc_l_b  <= adc_l_a;
        adc_r_a  <= adc_r_m;  adc_r_b  <= adc_r_a;
        peak_a   <= peak_m;   peak_b   <= peak_a;
    end

    assign scl_o = i2c[0];
    assign sda_o = i2c[1];

    always @(posedge aclk) begin
        if (!aresetn) begin
            ctrl <= 0; step <= 0; amp32 <= 0; i2c <= 32'h3; bvalid_r <= 0; peak_clr <= 0;
        end else begin
            peak_clr <= 1'b0;
            if (wr_en) begin
                bvalid_r <= 1'b1;
                case (s_awaddr[5:2])
                    4'd0: ctrl  <= s_wdata[2:0];
                    4'd1: step  <= s_wdata;
                    4'd2: amp32 <= s_wdata;
                    4'd6: peak_clr <= 1'b1;
                    4'd7: i2c   <= s_wdata;
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
                4'd0:    rdata_r <= {29'd0, ctrl};
                4'd1:    rdata_r <= step;
                4'd2:    rdata_r <= amp32;
                4'd3:    rdata_r <= frames_b;
                4'd4:    rdata_r <= adc_l_b;
                4'd5:    rdata_r <= adc_r_b;
                4'd6:    rdata_r <= peak_b;
                4'd7:    rdata_r <= {23'd0, sda_i, 6'd0, i2c[1:0]};
                default: rdata_r <= 0;
            endcase
        end else if (rvalid_r && s_rready) begin
            rvalid_r <= 1'b0;
        end
    end

    // ---------------- I2S master on MCLK ----------------
    // One frame = 256 MCLK cycles = 64 BCLK periods = 2 x 32-bit slots.
    // BCLK is cnt[1]; LRCLK and PBDAT change on the falling BCLK edge, i.e. they
    // are registered when cnt[1:0] == 3 so they appear as cnt wraps to xx00.
    reg  [7:0]  cnt = 8'd0;
    wire [7:0]  nxt = cnt + 8'd1;
    wire        nchan = nxt[7];             // channel of the slot starting next
    wire [4:0]  npos  = nxt[6:2];           // bit position in that slot
    wire        chan  = cnt[7];
    wire [4:0]  pos   = cnt[6:2];

    reg         en_s1 = 1'b0, en_s2 = 1'b0;
    reg  [2:0]  ctrl_s = 3'd0;
    reg  [31:0] step_s = 32'd0, amp_s = 32'd0;
    always @(posedge mclk) begin
        ctrl_s <= ctrl; step_s <= step; amp_s <= amp32;
        en_s1 <= ctrl[0]; en_s2 <= en_s1;
    end

    reg  [31:0] phase = 32'd0;
    reg  [15:0] left_w = 16'd0, right_w = 16'd0;
    reg  [31:0] frames = 32'd0;
    reg  [15:0] adc_l = 16'd0, adc_r = 16'd0;
    reg  [15:0] peak = 16'd0;
    reg  [15:0] rx_sh = 16'd0;
    reg         peak_clr_s1 = 1'b0, peak_clr_s2 = 1'b0;
    reg         lr_r = 1'b0, dat_r = 1'b0;

    // triangle in [-amp, +amp)
    wire [14:0]        tri_u  = phase[31] ? ~phase[30:16] : phase[30:16];   // 0..32767
    wire signed [16:0] tri_s  = $signed({2'b00, tri_u}) - 17'sd16384;        // -16384..16383
    wire signed [31:0] scaled = (tri_s * $signed({1'b0, amp_s[14:0]})) >>> 14;

    wire [15:0] rx_word = {rx_sh[14:0], recdat};

    always @(posedge mclk) begin
        peak_clr_s1 <= peak_clr; peak_clr_s2 <= peak_clr_s1;
        if (peak_clr_s2) peak <= 16'd0;

        if (!en_s2) begin
            cnt <= 8'd0; lr_r <= 1'b0; dat_r <= 1'b0;
            frames <= 32'd0; phase <= 32'd0;
        end else begin
            cnt <= nxt;
            // choose the next sample pair as the frame wraps
            if (cnt == 8'd255) begin
                frames <= frames + 32'd1;
                phase  <= phase + step_s;
                if (ctrl_s[2]) begin
                    left_w <= adc_l; right_w <= adc_r;
                end else if (ctrl_s[1]) begin
                    left_w <= scaled[15:0]; right_w <= scaled[15:0];
                end else begin
                    left_w <= 16'd0; right_w <= 16'd0;
                end
            end
            if (cnt[1:0] == 2'd3) begin
                lr_r  <= nchan;
                dat_r <= (npos >= 5'd1 && npos <= 5'd16)
                         ? (nchan ? right_w[16 - npos] : left_w[16 - npos]) : 1'b0;
            end
            // ADC: sample the line just before the rising BCLK edge
            if (cnt[1:0] == 2'd1 && pos >= 5'd1 && pos <= 5'd16) begin
                rx_sh <= rx_word;
                if (pos == 5'd16) begin
                    if (!chan) begin
                        adc_l <= rx_word;
                        if ((rx_word ^ {16{rx_word[15]}}) > peak)
                            peak <= rx_word ^ {16{rx_word[15]}};
                    end else begin
                        adc_r <= rx_word;
                    end
                end
            end
        end
    end

    assign bclk  = en_s2 ? cnt[1] : 1'b0;
    assign lrclk = lr_r;
    assign pbdat = dat_r;
    assign frames_m = frames;
    assign adc_l_m  = {{16{adc_l[15]}}, adc_l};
    assign adc_r_m  = {{16{adc_r[15]}}, adc_r};
    assign peak_m   = {16'd0, peak};
endmodule
