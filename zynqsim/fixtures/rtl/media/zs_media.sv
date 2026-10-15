`timescale 1ns / 1ps
// zs_media.sv: pin-level models of the Zybo Z7 audio codec (SSM2603) and HDMI
// connectors for ZynqSim testbenches.
//
// They run inside the RTL simulator and exchange *samples* and *video lines*
// with the host through the `zs_*` DPI functions of the xsim-dpi library, so the
// host sees 48 kHz PCM frames and RGB lines instead of pin toggles.
//
//   zs_ssm2603      I2S DAC receiver, I2S ADC transmitter, I2C slave (0x1A)
//   zs_hdmi_tx_sink pixel bus -> host video lines
//   zs_hdmi_rx_src  host image -> pixel bus with VGA-style timing

// ---------------------------------------------------------------------------
// SSM2603 audio codec.
//   Control: write-only I2C at 7-bit address 0x1A; a write is two data bytes
//            {reg[6:0], value[8]} {value[7:0]}.
//   Audio:   16-bit MSB-first I2S, the PL is master (BCLK/LRCLK). LRCLK low is
//            the left channel. DAC data is applied through the digital mute and
//            headphone volume registers before it is reported to the host.
// ---------------------------------------------------------------------------
module zs_ssm2603 (
    input  wire bclk,
    input  wire lrclk,      // PBLRC and RECLRC are the same signal here
    input  wire dac_data,   // PBDAT: PL -> codec
    output reg  adc_data,   // RECDAT: codec -> PL
    input  wire scl,
    inout  wire sda
);
    import "DPI-C" function void zs_audio_tx_frame(input int left, input int right, input longint now_ns);
    import "DPI-C" function void zs_audio_rx_frame(output int left, output int right, input longint now_ns);
    import "DPI-C" function void zs_codec_reg(input int reg_no, input int value, input longint now_ns);

    // ---- register file (9-bit) ----
    reg [8:0] regs [0:15];
    integer ri;
    initial begin
        for (ri = 0; ri < 16; ri = ri + 1) regs[ri] = 9'd0;
        regs[2] = 9'h079;   // left headphone volume: 0 dB
        regs[3] = 9'h079;
        regs[6] = 9'h0FF;   // everything powered down until firmware enables it
    end
    wire active  = regs[9][0];
    wire dacmute = regs[5][3];
    wire dac_off = regs[6][3] | regs[6][4];       // DACPD / OUTPD

    function integer gain_q15(input [6:0] vol);
        // 1 dB per step, 0 dB at 0x79; below 0x30 is mute. Q15 fixed point.
        real db;
        begin
            if (vol < 7'h30) gain_q15 = 0;
            else begin
                db = $itor(vol) - 121.0;
                gain_q15 = $rtoi($pow(10.0, db / 20.0) * 32768.0);
                if (gain_q15 > 32768) gain_q15 = 32768;
            end
        end
    endfunction

    // ---- DAC: I2S receiver (sample on rising BCLK) ----
    reg        lr_prev = 1'b1;
    reg        chan = 1'b0;
    integer    bitcnt = 16;
    reg [15:0] shreg = 16'd0;
    reg [15:0] left_w = 16'd0;
    integer    gl, gr, sl, sr;

    always @(posedge bclk) begin
        if (lrclk !== lr_prev) begin
            lr_prev <= lrclk;
            chan    <= lrclk;     // low = left, high = right
            bitcnt  <= 0;         // the bit sampled at this edge is the previous word's LSB
        end else if (bitcnt < 16) begin
            shreg  <= {shreg[14:0], dac_data};
            bitcnt <= bitcnt + 1;
            if (bitcnt == 15) begin
                if (!chan) left_w <= {shreg[14:0], dac_data};
                else begin
                    gl = $signed(left_w);
                    gr = $signed({shreg[14:0], dac_data});
                    if (!active || dacmute || dac_off) begin
                        sl = 0; sr = 0;
                    end else begin
                        sl = (gl * gain_q15(regs[2][6:0])) >>> 15;
                        sr = (gr * gain_q15(regs[3][6:0])) >>> 15;
                    end
                    zs_audio_tx_frame(sl, sr, $time);
                end
            end
        end
    end

    // ---- ADC: I2S transmitter (change on falling BCLK) ----
    reg        alr_prev = 1'b1;
    integer    abit = 32;
    reg [15:0] aleft = 16'd0, aright = 16'd0;
    reg [15:0] aword = 16'd0;
    integer    fl, fr;
    initial adc_data = 1'b0;

    always @(negedge bclk) begin
        if (lrclk !== alr_prev) begin
            alr_prev = lrclk;
            abit = 0;
            if (!lrclk) begin
                // start of a frame: fetch the next stereo frame from the host
                zs_audio_rx_frame(fl, fr, $time);
                aleft  = fl[15:0];
                aright = fr[15:0];
            end
            aword = lrclk ? aright : aleft;
            adc_data <= 1'b0;                 // one bit of delay after the edge
        end else begin
            if (abit < 16) adc_data <= aword[15 - abit];
            else           adc_data <= 1'b0;
            abit = abit + 1;
        end
    end

    // ---- I2C slave, write-only ----
    // START/STOP are SDA edges while SCL is high; bits are sampled on rising
    // SCL; ACKs are driven from the falling SCL after the eighth bit.
    reg        sda_low = 1'b0;               // we pull SDA low
    assign sda = sda_low ? 1'b0 : 1'bz;
    integer    i2c_state = 0;                // 0 idle, 1 address, 2 byte 1, 3 byte 2
    integer    nbits = 0;
    reg [7:0]  sh = 8'd0;
    reg [7:0]  b1 = 8'd0;
    reg        ack_phase = 1'b0;
    reg        addr_ok = 1'b0;
    integer    k;

    always @(negedge sda) begin
        if (scl === 1'b1) begin               // START
            i2c_state = 1; nbits = 0; sh = 8'd0; sda_low = 1'b0; ack_phase = 1'b0;
        end
    end

    always @(posedge sda) begin
        if (scl === 1'b1) begin               // STOP
            i2c_state = 0; sda_low = 1'b0; ack_phase = 1'b0;
        end
    end

    always @(posedge scl) begin
        if (i2c_state != 0 && !ack_phase && nbits < 8) begin
            sh = {sh[6:0], sda};
            nbits = nbits + 1;
        end
    end

    always @(negedge scl) begin
        if (i2c_state != 0) begin
            if (ack_phase) begin
                // end of the ACK clock: release SDA and take the byte
                sda_low = 1'b0;
                ack_phase = 1'b0;
                nbits = 0;
                case (i2c_state)
                    1: i2c_state = addr_ok ? 2 : 0;
                    2: begin b1 = sh; i2c_state = 3; end
                    default: begin
                        if (b1[7:1] < 7'd16) begin
                            regs[b1[7:1]] = {b1[0], sh};
                            zs_codec_reg(b1[7:1], {b1[0], sh}, $time);
                            if (b1[7:1] == 7'd15)
                                for (k = 0; k < 15; k = k + 1) regs[k] = 9'd0;
                        end
                        i2c_state = 0;
                    end
                endcase
                sh = 8'd0;
            end else if (nbits == 8) begin
                // eight bits clocked in: acknowledge
                ack_phase = 1'b1;
                if (i2c_state == 1) begin
                    addr_ok = (sh[7:1] == 7'h1A) && !sh[0];
                    sda_low = addr_ok;
                end else begin
                    sda_low = 1'b1;
                end
            end
        end
    end
endmodule

// ---------------------------------------------------------------------------
// HDMI TX connector: forwards the PL's pixel bus to the host.
// ---------------------------------------------------------------------------
module zs_hdmi_tx_sink #(
    parameter VS_ACTIVE_LOW = 1
) (
    input wire        pclk,
    input wire        de,
    input wire        hs,
    input wire        vs,
    input wire [23:0] rgb
);
    import "DPI-C" function void zs_video_tx_pixel(input int de, input int vs, input int rgb, input longint now_ns);
    wire vs_active = VS_ACTIVE_LOW ? ~vs : vs;
    always @(posedge pclk) zs_video_tx_pixel(de, vs_active, {8'h00, rgb}, $time);
endmodule

// ---------------------------------------------------------------------------
// HDMI RX connector: presents the host's video source to the PL as a pixel
// bus. `pclk` is derived from `ref_clk` (divide by 4) so the PL can sample it
// without a clock-domain crossing.
// ---------------------------------------------------------------------------
module zs_hdmi_rx_src #(
    parameter H_ACT = 320, H_FP = 8, H_SYNC = 16, H_BP = 24,
    parameter V_ACT = 240, V_FP = 2, V_SYNC = 2,  V_BP = 10
) (
    input  wire        ref_clk,
    input  wire        enable,
    output reg         pclk,
    output reg         de,
    output reg         hs,
    output reg         vs,
    output reg  [23:0] rgb
);
    import "DPI-C" function int zs_video_rx_line(input int frame, input int y, input int w, input int h, input longint now_ns);
    import "DPI-C" function int zs_video_rx_pixel(input int x);

    localparam H_TOT = H_ACT + H_FP + H_SYNC + H_BP;
    localparam V_TOT = V_ACT + V_FP + V_SYNC + V_BP;

    reg [1:0] div = 2'd0;
    integer   h_cnt = 0, v_cnt = 0, frame = 0, rc;

    initial begin
        pclk = 1'b0; de = 1'b0; hs = 1'b1; vs = 1'b1; rgb = 24'd0;
    end

    always @(posedge ref_clk) begin
        div <= div + 2'd1;
        pclk <= div[1];
        if (enable && div == 2'd3) begin
            // one pixel period elapsed: advance the raster
            if (h_cnt == H_TOT - 1) begin
                h_cnt = 0;
                v_cnt = (v_cnt == V_TOT - 1) ? 0 : v_cnt + 1;
                if (v_cnt == 0) frame = frame + 1;
            end else begin
                h_cnt = h_cnt + 1;
            end
            hs <= ~((h_cnt >= H_ACT + H_FP) && (h_cnt < H_ACT + H_FP + H_SYNC));
            vs <= ~((v_cnt >= V_ACT + V_FP) && (v_cnt < V_ACT + V_FP + V_SYNC));
            de <= (h_cnt < H_ACT) && (v_cnt < V_ACT);
            if (h_cnt == 0 && v_cnt < V_ACT)
                rc = zs_video_rx_line(frame, v_cnt, H_ACT, V_ACT, $time);
            if (h_cnt < H_ACT && v_cnt < V_ACT)
                rgb <= zs_video_rx_pixel(h_cnt) & 32'h00FF_FFFF;
            else
                rgb <= 24'd0;
        end
    end
endmodule
