`timescale 1ns / 1ps
// ILLEGAL on purpose (ADR-025 test fixture): a copy of my_math_ip whose RVALID pulses for
// one cycle after every write, although no read is pending and RREADY is low. The
// protocol checker must report `AXI4-Lite slave R VALID_DROP`: VALID fell before READY.
// Firmware: the math test (writes reg0), e.g. fixtures/elf/math_test.elf.
module illegal_lite_rvalid_drop #(
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
    input  wire                                  S_AXI_RREADY
);
    reg  [C_S_AXI_DATA_WIDTH-1:0] slv_reg0;
    reg                           bvalid;
    reg                           rvalid;
    reg                           spur;      // the spurious R beat
    reg  [C_S_AXI_DATA_WIDTH-1:0] rdata;

    wire write_en = S_AXI_AWVALID & S_AXI_WVALID & ~bvalid;
    wire read_en  = S_AXI_ARVALID & ~rvalid;

    assign S_AXI_AWREADY = write_en;
    assign S_AXI_WREADY  = write_en;
    assign S_AXI_BRESP   = 2'b00;
    assign S_AXI_BVALID  = bvalid;
    assign S_AXI_ARREADY = read_en;
    assign S_AXI_RDATA   = rdata;
    assign S_AXI_RRESP   = 2'b00;
    assign S_AXI_RVALID  = rvalid | spur;

    integer i;
    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            slv_reg0 <= {C_S_AXI_DATA_WIDTH{1'b0}};
            bvalid   <= 1'b0;
            spur     <= 1'b0;
        end else begin
            spur <= write_en;                 // one cycle after the write, no READY
            if (write_en) begin
                if (S_AXI_AWADDR[C_S_AXI_ADDR_WIDTH-1:2] == 2'd0) begin
                    for (i = 0; i < C_S_AXI_DATA_WIDTH/8; i = i + 1)
                        if (S_AXI_WSTRB[i])
                            slv_reg0[i*8 +: 8] <= S_AXI_WDATA[i*8 +: 8];
                end
                bvalid <= 1'b1;
            end else if (bvalid && S_AXI_BREADY) begin
                bvalid <= 1'b0;
            end
        end
    end

    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            rvalid <= 1'b0;
            rdata  <= {C_S_AXI_DATA_WIDTH{1'b0}};
        end else begin
            if (read_en) begin
                rdata  <= (S_AXI_ARADDR[C_S_AXI_ADDR_WIDTH-1:2] == 2'd0) ? slv_reg0 : 32'd0;
                rvalid <= 1'b1;
            end else if (rvalid && S_AXI_RREADY) begin
                rvalid <= 1'b0;
            end
        end
    end
endmodule
