`timescale 1ns / 1ps
// i2c_master: an AXI4-Lite I2C master (open drain, no clock stretching) for one
// Pmod port. Byte-level commands, like a PS I2C controller's command FIFO of
// depth one.
//
//   0x00 CMD     W  start a transfer (ignored while BUSY):
//                   bit0 START (also repeated START), bit1 STOP,
//                   bit2 WRITE (shift TXDATA out, sample the ACK),
//                   bit3 READ (shift 8 bits into RXDATA, answer with ACK, or
//                   NACK when bit4 is set). Order: START, byte, STOP.
//   0x04 TXDATA  RW [7:0]
//   0x08 RXDATA  R  [7:0] the last byte read
//   0x0C STATUS  R  bit0 BUSY, bit1 NACK (the last written byte was not ACKed)
//   0x10 DIV     RW clock cycles per quarter of an SCL period (reset 250:
//                   100 MHz / (4 * 250) = 100 kHz)
//   0x14 PINS    R  the eight pin levels as the board drives them (INT, CT, ...)
//
// Pins, Pmod order 1,2,3,4,7,8,9,10 = bit 0..7 (Digilent Pmod TMP2 / SCCB):
//   bit2 SCL, bit3 SDA; bits 0,1 (INT, CT) are inputs from the module.
// `pmod_ja_o` is what this master drives (1 = released); `pmod_ja_i` is what
// the module drives (1 = released). The bus is their wired AND.
module i2c_master (
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
    localparam IDLE = 2'd0, START = 2'd1, BITS = 2'd2, STOP = 2'd3;

    reg        bvalid_r, rvalid_r;
    reg [31:0] rdata_r;
    reg [7:0]  txdata, rxdata;
    reg [15:0] div;
    reg        nack;
    reg [4:0]  cmd;          // latched command flags
    reg [1:0]  st;
    reg [1:0]  q;            // quarter within a phase
    reg [3:0]  bitn;         // 0..7 data bits, 8 the ACK bit
    reg [7:0]  shift;
    reg [15:0] cnt;
    reg        scl, sda;     // what this master drives (1 = released)

    wire busy = (st != IDLE);
    wire tick = (cnt + 16'd1 >= div);
    wire bus_sda = sda & pmod_ja_i[3];

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

    assign pmod_ja_o = { 4'hF, sda, scl, 2'b11 };

    // The first phase after a command: START if asked, else the byte, else STOP.
    task begin_transfer(input [4:0] c);
        begin
            q <= 2'd0; bitn <= 4'd0; cnt <= 16'd0;
            if (c[0])                   st <= START;
            else if (c[2] | c[3])       st <= BITS;
            else                        st <= STOP;
            if (!c[0] && c[2])          shift <= txdata;
        end
    endtask

    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            bvalid_r <= 0; rvalid_r <= 0; rdata_r <= 0;
            txdata <= 0; rxdata <= 0; div <= 16'd250; nack <= 0;
            cmd <= 0; st <= IDLE; q <= 0; bitn <= 0; shift <= 0; cnt <= 0;
            scl <= 1'b1; sda <= 1'b1;
        end else begin
            if (wr) begin
                bvalid_r <= 1'b1;
                case (S_AXI_AWADDR[4:2])
                    3'd0: if (!busy) begin
                              cmd <= S_AXI_WDATA[4:0];
                              begin_transfer(S_AXI_WDATA[4:0]);
                          end
                    3'd1: txdata <= S_AXI_WDATA[7:0];
                    3'd4: div    <= S_AXI_WDATA[15:0];
                    default: ;
                endcase
            end else if (bvalid_r && S_AXI_BREADY) begin
                bvalid_r <= 1'b0;
            end

            if (rd) begin
                rvalid_r <= 1'b1;
                case (S_AXI_ARADDR[4:2])
                    3'd1:    rdata_r <= {24'd0, txdata};
                    3'd2:    rdata_r <= {24'd0, rxdata};
                    3'd3:    rdata_r <= {30'd0, nack, busy};
                    3'd4:    rdata_r <= {16'd0, div};
                    3'd5:    rdata_r <= {24'd0, pmod_ja_i};
                    default: rdata_r <= 32'd0;
                endcase
            end else if (rvalid_r && S_AXI_RREADY) begin
                rvalid_r <= 1'b0;
            end

            // engine: one step per `div` cycles
            if (busy && !(wr && S_AXI_AWADDR[4:2] == 3'd0)) begin
                if (!tick) begin
                    cnt <= cnt + 16'd1;
                end else begin
                    cnt <= 16'd0;
                    case (st)
                    START: begin
                        // release SDA, release SCL, SDA falls (START), SCL falls
                        case (q)
                            2'd0: begin sda <= 1'b1; q <= 2'd1; end
                            2'd1: begin scl <= 1'b1; q <= 2'd2; end
                            2'd2: begin sda <= 1'b0; q <= 2'd3; end
                            2'd3: begin
                                scl <= 1'b0; q <= 2'd0;
                                if (cmd[2] | cmd[3]) begin
                                    st <= BITS; bitn <= 4'd0;
                                    if (cmd[2]) shift <= txdata;
                                end else st <= STOP;
                            end
                        endcase
                    end
                    BITS: begin
                        case (q)
                            2'd0: begin
                                // SCL is low: put the bit on SDA
                                if (bitn == 4'd8)      sda <= cmd[3] ? cmd[4] : 1'b1;
                                else if (cmd[2])       sda <= shift[7];
                                else                   sda <= 1'b1;
                                q <= 2'd1;
                            end
                            2'd1: begin scl <= 1'b1; q <= 2'd2; end
                            2'd2: begin
                                // SCL high and stable: sample
                                if (bitn == 4'd8) begin
                                    if (cmd[2]) nack <= bus_sda;
                                end else if (cmd[3]) begin
                                    shift <= {shift[6:0], bus_sda};
                                end
                                q <= 2'd3;
                            end
                            2'd3: begin
                                scl <= 1'b0; q <= 2'd0;
                                if (bitn == 4'd8) begin
                                    if (cmd[3]) rxdata <= shift;
                                    sda <= 1'b1;
                                    if (cmd[1]) begin st <= STOP; end
                                    else st <= IDLE;
                                end else begin
                                    bitn <= bitn + 4'd1;
                                    if (cmd[2]) shift <= {shift[6:0], 1'b0};
                                end
                            end
                        endcase
                    end
                    STOP: begin
                        // SDA low, SCL rises, SDA rises (STOP)
                        case (q)
                            2'd0: begin sda <= 1'b0; q <= 2'd1; end
                            2'd1: begin scl <= 1'b1; q <= 2'd2; end
                            2'd2: begin sda <= 1'b1; q <= 2'd3; end
                            2'd3: begin q <= 2'd0; st <= IDLE; end
                        endcase
                    end
                    default: st <= IDLE;
                    endcase
                end
            end
        end
    end
endmodule
