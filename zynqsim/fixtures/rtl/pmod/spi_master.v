`timescale 1ns / 1ps
// spi_master: an AXI4-Lite SPI master with MISO for one Pmod port, any of the
// four SPI modes, MSB first, 8-bit words, chip select under software control.
//
//   0x00 DATA    W  start shifting this byte out (ignored while BUSY)
//   0x04 STATUS  R  bit0 BUSY
//   0x08 CTRL    RW bit0 CPOL, bit1 CPHA, bit2 CS_N (1 = deselected)
//                   reset value 0x04
//   0x0C DIV     RW SCLK half period in aclk cycles (reset 50: 1 MHz)
//   0x10 RXDATA  R  the byte clocked in by the last transfer
//   0x14 PINS    R  the eight pin levels as the module drives them
//
// Pins, Pmod order 1,2,3,4,7,8,9,10 = bit 0..7 (Digilent Pmod ACL / ALS):
//   bit0 CS, bit1 MOSI, bit2 MISO (input), bit3 SCLK, bit4 INT1 (input),
//   bit5 INT2 (input).
// `pmod_ja_o` is what this master drives; `pmod_ja_i` is what the module
// drives (1 = released); MISO is `pmod_ja_i[2]`.
module spi_master (
    input  wire        S_AXI_ACLK,
    input  wire        S_AXI_ARESETN,
    input  wire [4:0]  S_AXI_AWADDR,
    input  wire        S_AXI_AWVALID,
    output wire        S_AXI_AWREADY,
    input  wire [31:0] S_AXI_WDATA,
    input  wire [3:0]  S_AXI_WSTRB,
    input  wire        S_AXI_WVALID,
    output wire        S_AXI_WREADY,
    output wire [1:0]  S_AXI_BRESP,
    output wire        S_AXI_BVALID,
    input  wire        S_AXI_BREADY,
    input  wire [4:0]  S_AXI_ARADDR,
    input  wire        S_AXI_ARVALID,
    output wire        S_AXI_ARREADY,
    output wire [31:0] S_AXI_RDATA,
    output wire [1:0]  S_AXI_RRESP,
    output wire        S_AXI_RVALID,
    input  wire        S_AXI_RREADY,
    output wire [7:0]  pmod_ja_o,
    input  wire [7:0]  pmod_ja_i
);
    reg        bvalid_r, rvalid_r;
    reg [31:0] rdata_r;
    reg [2:0]  ctrl;
    reg [15:0] div;
    reg        busy;
    reg [7:0]  tx, rx, rxdata;
    reg [4:0]  edge_n;       // 0..15: the toggle of SCLK in progress
    reg [15:0] cnt;
    reg        sclk_r, mosi;

    wire cpol = ctrl[0];
    wire cpha = ctrl[1];
    wire sclk = busy ? sclk_r : cpol;
    wire leading = ~edge_n[0];
    wire sample_edge = (leading ~^ ~cpha);   // CPHA 0: leading, CPHA 1: trailing

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

    assign pmod_ja_o = { 2'b11, 1'b1, 1'b1, sclk, 1'b1, mosi, ctrl[2] };

    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            bvalid_r <= 0; rvalid_r <= 0; rdata_r <= 0;
            ctrl <= 3'b100; div <= 16'd50;
            busy <= 0; tx <= 0; rx <= 0; rxdata <= 0; edge_n <= 0; cnt <= 0;
            sclk_r <= 0; mosi <= 0;
        end else begin
            if (wr) begin
                bvalid_r <= 1'b1;
                case (S_AXI_AWADDR[4:2])
                    3'd0: if (!busy) begin
                              tx <= S_AXI_WDATA[7:0]; busy <= 1'b1;
                              mosi <= S_AXI_WDATA[7]; edge_n <= 0; cnt <= 0;
                              sclk_r <= cpol;
                          end
                    3'd2: ctrl <= S_AXI_WDATA[2:0];
                    3'd3: div  <= S_AXI_WDATA[15:0];
                    default: ;
                endcase
            end else if (bvalid_r && S_AXI_BREADY) begin
                bvalid_r <= 1'b0;
            end

            if (rd) begin
                rvalid_r <= 1'b1;
                case (S_AXI_ARADDR[4:2])
                    3'd1:    rdata_r <= {31'd0, busy};
                    3'd2:    rdata_r <= {29'd0, ctrl};
                    3'd3:    rdata_r <= {16'd0, div};
                    3'd4:    rdata_r <= {24'd0, rxdata};
                    3'd5:    rdata_r <= {24'd0, pmod_ja_i};
                    default: rdata_r <= 32'd0;
                endcase
            end else if (rvalid_r && S_AXI_RREADY) begin
                rvalid_r <= 1'b0;
            end

            // shifter: one SCLK toggle per `div` cycles, 16 toggles per byte
            if (busy && !(wr && S_AXI_AWADDR[4:2] == 3'd0)) begin
                if (cnt + 16'd1 >= div) begin
                    cnt <= 0;
                    sclk_r <= ~sclk_r;
                    if (sample_edge) rx <= {rx[6:0], pmod_ja_i[2]};
                    // The other edge shifts: CPHA 0 on the trailing edge,
                    // CPHA 1 on the leading edge.
                    if (!sample_edge) begin
                        if (cpha) begin
                            if (edge_n != 0) begin
                                tx   <= {tx[6:0], 1'b0};
                                mosi <= tx[6];
                            end else begin
                                mosi <= tx[7];
                            end
                        end else begin
                            tx   <= {tx[6:0], 1'b0};
                            mosi <= tx[6];
                        end
                    end
                    if (edge_n == 5'd15) begin
                        busy <= 1'b0; edge_n <= 0;
                        // The last sample edge of this byte: CPHA 0 samples on
                        // a leading edge (n = 14), CPHA 1 on the trailing edge
                        // that ends the byte (n = 15), which is this one.
                        rxdata <= cpha ? {rx[6:0], pmod_ja_i[2]} : rx;
                    end else begin
                        edge_n <= edge_n + 5'd1;
                    end
                end else begin
                    cnt <= cnt + 16'd1;
                end
            end
        end
    end
endmodule
