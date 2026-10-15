`timescale 1ns / 1ps
// oled_spi: an AXI4-Lite SPI master for the Digilent Pmod OLED (SSD1306, 128x32)
// on Pmod JA, and nothing else.
//
//   0x00 DATA    W  start shifting this byte out (ignored while BUSY)
//   0x04 STATUS  R  bit0 BUSY
//   0x08 CTRL    RW bit0 CS (1 = deselected), bit1 DC (1 = data), bit2 RES_N,
//                   bit3 VBATC, bit4 VDDC           (reset value 0x05)
//   0x0C DIV     RW SCLK half period in aclk cycles (reset value 4)
//
// pmod_ja[7:0] = { VDDC, VBATC, RES, D/C, SCLK, -, SDIN, CS }  (Pmod pins 1,2,3,4,7,8,9,10)
// SPI mode 0, MSB first.
module oled_spi (
    input  wire        S_AXI_ACLK,
    input  wire        S_AXI_ARESETN,
    input  wire [3:0]  S_AXI_AWADDR,
    input  wire        S_AXI_AWVALID,
    output wire        S_AXI_AWREADY,
    input  wire [31:0] S_AXI_WDATA,
    input  wire [3:0]  S_AXI_WSTRB,
    input  wire        S_AXI_WVALID,
    output wire        S_AXI_WREADY,
    output wire [1:0]  S_AXI_BRESP,
    output wire        S_AXI_BVALID,
    input  wire        S_AXI_BREADY,
    input  wire [3:0]  S_AXI_ARADDR,
    input  wire        S_AXI_ARVALID,
    output wire        S_AXI_ARREADY,
    output wire [31:0] S_AXI_RDATA,
    output wire [1:0]  S_AXI_RRESP,
    output wire        S_AXI_RVALID,
    input  wire        S_AXI_RREADY,
    output wire [7:0]  pmod_ja
);
    reg        bvalid_r, rvalid_r;
    reg [31:0] rdata_r;
    reg [4:0]  ctrl;
    reg [15:0] div;
    reg        busy;
    reg [7:0]  shift;
    reg [3:0]  bits;
    reg [15:0] cnt;
    reg        sclk, mosi;

    wire wr = S_AXI_AWVALID & S_AXI_WVALID & ~bvalid_r;
    wire rd = S_AXI_ARVALID & ~rvalid_r;
    assign S_AXI_AWREADY = wr;
    assign S_AXI_WREADY  = wr;
    assign S_AXI_BRESP   = 2'b00;
    assign S_AXI_BVALID  = bvalid_r;
    assign S_AXI_ARREADY = rd;
    assign S_AXI_RDATA   = rdata_r;
    assign S_AXI_RRESP   = 2'b00;
    assign S_AXI_RVALID  = rvalid_r;

    assign pmod_ja = { ctrl[4], ctrl[3], ctrl[2], ctrl[1], sclk, 1'b0, mosi, ctrl[0] };

    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            bvalid_r <= 0; rvalid_r <= 0; rdata_r <= 0;
            ctrl <= 5'b00101; div <= 16'd4;
            busy <= 0; shift <= 0; bits <= 0; cnt <= 0; sclk <= 0; mosi <= 0;
        end else begin
            if (wr) begin
                bvalid_r <= 1'b1;
                case (S_AXI_AWADDR[3:2])
                    2'd0: if (!busy) begin
                              shift <= S_AXI_WDATA[7:0]; bits <= 4'd8; busy <= 1'b1;
                              mosi <= S_AXI_WDATA[7]; cnt <= 0; sclk <= 1'b0;
                          end
                    2'd2: ctrl <= S_AXI_WDATA[4:0];
                    2'd3: div  <= S_AXI_WDATA[15:0];
                    default: ;
                endcase
            end else if (bvalid_r && S_AXI_BREADY) begin
                bvalid_r <= 1'b0;
            end

            if (rd) begin
                rvalid_r <= 1'b1;
                case (S_AXI_ARADDR[3:2])
                    2'd1:    rdata_r <= {31'd0, busy};
                    2'd2:    rdata_r <= {27'd0, ctrl};
                    2'd3:    rdata_r <= {16'd0, div};
                    default: rdata_r <= 32'd0;
                endcase
            end else if (rvalid_r && S_AXI_RREADY) begin
                rvalid_r <= 1'b0;
            end

            // shifter
            if (busy && !(wr && S_AXI_AWADDR[3:2] == 2'd0)) begin
                if (cnt + 1 >= div) begin
                    cnt <= 0;
                    if (!sclk) begin
                        sclk <= 1'b1;                 // rising edge: the device samples
                    end else begin
                        sclk <= 1'b0;                 // falling edge: next bit
                        if (bits == 4'd1) begin
                            busy <= 1'b0; bits <= 0;
                        end else begin
                            bits  <= bits - 4'd1;
                            shift <= {shift[6:0], 1'b0};
                            mosi  <= shift[6];
                        end
                    end
                end else begin
                    cnt <= cnt + 1;
                end
            end
        end
    end
endmodule
