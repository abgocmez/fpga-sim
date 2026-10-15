`timescale 1ns / 1ps
// irq_ip: a minimal AXI4-Lite slave with one level-high interrupt output.
//   0x00 CTRL     RW  bit0 = interrupt enable
//   0x04 TRIGGER  WO  any write raises an event (pending)
//   0x08 STATUS   RO  bit0 = pending, [31:8] = events raised so far
//   0x0C ACK      WO  any write clears pending
// irq = pending & CTRL[0]. Every transaction completes with OKAY.
module irq_ip #(
    parameter integer C_S_AXI_DATA_WIDTH = 32,
    parameter integer C_S_AXI_ADDR_WIDTH = 4
) (
    input  wire                                  S_AXI_ACLK,
    input  wire                                  S_AXI_ARESETN,
    input  wire [C_S_AXI_ADDR_WIDTH-1:0]         S_AXI_AWADDR,
    input  wire [2:0]                            S_AXI_AWPROT,
    input  wire                                  S_AXI_AWVALID,
    output wire                                  S_AXI_AWREADY,
    input  wire [C_S_AXI_DATA_WIDTH-1:0]         S_AXI_WDATA,
    input  wire [(C_S_AXI_DATA_WIDTH/8)-1:0]     S_AXI_WSTRB,
    input  wire                                  S_AXI_WVALID,
    output wire                                  S_AXI_WREADY,
    output wire [1:0]                            S_AXI_BRESP,
    output wire                                  S_AXI_BVALID,
    input  wire                                  S_AXI_BREADY,
    input  wire [C_S_AXI_ADDR_WIDTH-1:0]         S_AXI_ARADDR,
    input  wire [2:0]                            S_AXI_ARPROT,
    input  wire                                  S_AXI_ARVALID,
    output wire                                  S_AXI_ARREADY,
    output wire [C_S_AXI_DATA_WIDTH-1:0]         S_AXI_RDATA,
    output wire [1:0]                            S_AXI_RRESP,
    output wire                                  S_AXI_RVALID,
    input  wire                                  S_AXI_RREADY,
    output wire                                  irq
);
    reg        ctrl_en;
    reg        pending;
    reg [23:0] events;
    reg        bvalid_r, rvalid_r;
    reg [31:0] rdata_r;

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
    assign irq           = pending & ctrl_en;

    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            ctrl_en <= 1'b0; pending <= 1'b0; events <= 24'd0;
            bvalid_r <= 1'b0; rvalid_r <= 1'b0; rdata_r <= 32'd0;
        end else begin
            if (wr) begin
                bvalid_r <= 1'b1;
                case (S_AXI_AWADDR[3:2])
                    2'd0: ctrl_en <= S_AXI_WDATA[0];
                    2'd1: begin pending <= 1'b1; events <= events + 24'd1; end
                    2'd3: pending <= 1'b0;
                    default: ;
                endcase
            end else if (bvalid_r && S_AXI_BREADY) begin
                bvalid_r <= 1'b0;
            end

            if (rd) begin
                rvalid_r <= 1'b1;
                case (S_AXI_ARADDR[3:2])
                    2'd0:    rdata_r <= {31'd0, ctrl_en};
                    2'd2:    rdata_r <= {events, 7'd0, pending};
                    default: rdata_r <= 32'd0;
                endcase
            end else if (rvalid_r && S_AXI_RREADY) begin
                rvalid_r <= 1'b0;
            end
        end
    end
endmodule
