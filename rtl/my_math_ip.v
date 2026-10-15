`timescale 1ns / 1ps
// my_math_ip: minimal AXI4-Lite slave.
//   0x00 slv_reg0  RW  32-bit operand
//   0x04 slv_reg1  RO  slv_reg0 * 2 (truncated to 32 bits)
// Every transaction completes with OKAY (2'b00). Writes to 0x04 are ignored
// but still acknowledged; reads of unmapped offsets return 0.
module my_math_ip #(
    parameter integer C_S_AXI_DATA_WIDTH = 32,
    parameter integer C_S_AXI_ADDR_WIDTH = 4
) (
    (* X_INTERFACE_INFO = "xilinx.com:signal:clock:1.0 S_AXI_ACLK CLK" *)
    (* X_INTERFACE_PARAMETER = "ASSOCIATED_BUSIF S_AXI, ASSOCIATED_RESET S_AXI_ARESETN" *)
    input  wire                                  S_AXI_ACLK,
    (* X_INTERFACE_INFO = "xilinx.com:signal:reset:1.0 S_AXI_ARESETN RST" *)
    (* X_INTERFACE_PARAMETER = "POLARITY ACTIVE_LOW" *)
    input  wire                                  S_AXI_ARESETN,

    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWADDR" *)
    input  wire [C_S_AXI_ADDR_WIDTH-1:0]         S_AXI_AWADDR,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWPROT" *)
    input  wire [2:0]                            S_AXI_AWPROT,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWVALID" *)
    input  wire                                  S_AXI_AWVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWREADY" *)
    output wire                                  S_AXI_AWREADY,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *)
    input  wire [C_S_AXI_DATA_WIDTH-1:0]         S_AXI_WDATA,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WSTRB" *)
    input  wire [(C_S_AXI_DATA_WIDTH/8)-1:0]     S_AXI_WSTRB,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WVALID" *)
    input  wire                                  S_AXI_WVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WREADY" *)
    output wire                                  S_AXI_WREADY,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BRESP" *)
    output wire [1:0]                            S_AXI_BRESP,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BVALID" *)
    output wire                                  S_AXI_BVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BREADY" *)
    input  wire                                  S_AXI_BREADY,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARADDR" *)
    input  wire [C_S_AXI_ADDR_WIDTH-1:0]         S_AXI_ARADDR,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARPROT" *)
    input  wire [2:0]                            S_AXI_ARPROT,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARVALID" *)
    input  wire                                  S_AXI_ARVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARREADY" *)
    output wire                                  S_AXI_ARREADY,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *)
    output wire [C_S_AXI_DATA_WIDTH-1:0]         S_AXI_RDATA,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RRESP" *)
    output wire [1:0]                            S_AXI_RRESP,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RVALID" *)
    output wire                                  S_AXI_RVALID,
    (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RREADY" *)
    input  wire                                  S_AXI_RREADY
);

    reg  [C_S_AXI_DATA_WIDTH-1:0] slv_reg0;
    wire [C_S_AXI_DATA_WIDTH-1:0] slv_reg1 = slv_reg0 * 2;

    reg                           bvalid;
    reg                           rvalid;
    reg  [C_S_AXI_DATA_WIDTH-1:0] rdata;

    // Accept AW and W together, one write in flight at a time.
    wire write_en = S_AXI_AWVALID & S_AXI_WVALID & ~bvalid;
    wire read_en  = S_AXI_ARVALID & ~rvalid;

    assign S_AXI_AWREADY = write_en;
    assign S_AXI_WREADY  = write_en;
    assign S_AXI_BRESP   = 2'b00;
    assign S_AXI_BVALID  = bvalid;
    assign S_AXI_ARREADY = read_en;
    assign S_AXI_RDATA   = rdata;
    assign S_AXI_RRESP   = 2'b00;
    assign S_AXI_RVALID  = rvalid;

    integer i;
    always @(posedge S_AXI_ACLK) begin
        if (!S_AXI_ARESETN) begin
            slv_reg0 <= {C_S_AXI_DATA_WIDTH{1'b0}};
            bvalid   <= 1'b0;
        end else begin
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
                case (S_AXI_ARADDR[C_S_AXI_ADDR_WIDTH-1:2])
                    2'd0:    rdata <= slv_reg0;
                    2'd1:    rdata <= slv_reg1;
                    default: rdata <= {C_S_AXI_DATA_WIDTH{1'b0}};
                endcase
                rvalid <= 1'b1;
            end else if (rvalid && S_AXI_RREADY) begin
                rvalid <= 1'b0;
            end
        end
    end

endmodule
