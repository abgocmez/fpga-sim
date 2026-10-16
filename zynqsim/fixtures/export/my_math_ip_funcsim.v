// Copyright 1986-2022 Xilinx, Inc. All Rights Reserved.
// --------------------------------------------------------------------------------
// Tool Version: Vivado v.2022.1 (win64) Build 3526262 Mon Apr 18 15:48:16 MDT 2022
// Date        : Fri Oct 16 01:59:40 2026
// Host        : batik running 64-bit major release  (build 9200)
// Command     : write_verilog -mode funcsim -force C:/zsx_exp/out/my_math_ip_funcsim.v
// Design      : my_math_ip
// Purpose     : This verilog netlist is a functional simulation representation of the design and should not be modified
//               or synthesized. This netlist cannot be used for SDF annotated simulation.
// Device      : xc7z020clg400-1
// --------------------------------------------------------------------------------
`timescale 1 ps / 1 ps

(* C_S_AXI_ADDR_WIDTH = "4" *) (* C_S_AXI_DATA_WIDTH = "32" *) 
(* NotValidForBitStream *)
module my_math_ip
   (S_AXI_ACLK,
    S_AXI_ARESETN,
    S_AXI_AWADDR,
    S_AXI_AWPROT,
    S_AXI_AWVALID,
    S_AXI_AWREADY,
    S_AXI_WDATA,
    S_AXI_WSTRB,
    S_AXI_WVALID,
    S_AXI_WREADY,
    S_AXI_BRESP,
    S_AXI_BVALID,
    S_AXI_BREADY,
    S_AXI_ARADDR,
    S_AXI_ARPROT,
    S_AXI_ARVALID,
    S_AXI_ARREADY,
    S_AXI_RDATA,
    S_AXI_RRESP,
    S_AXI_RVALID,
    S_AXI_RREADY);
  (* X_INTERFACE_INFO = "xilinx.com:signal:clock:1.0 S_AXI_ACLK CLK" *) (* X_INTERFACE_PARAMETER = "ASSOCIATED_BUSIF S_AXI, ASSOCIATED_RESET S_AXI_ARESETN" *) input S_AXI_ACLK;
  (* X_INTERFACE_INFO = "xilinx.com:signal:reset:1.0 S_AXI_ARESETN RST" *) (* X_INTERFACE_PARAMETER = "POLARITY ACTIVE_LOW" *) input S_AXI_ARESETN;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWADDR" *) input [3:0]S_AXI_AWADDR;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWPROT" *) input [2:0]S_AXI_AWPROT;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWVALID" *) input S_AXI_AWVALID;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWREADY" *) output S_AXI_AWREADY;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) input [31:0]S_AXI_WDATA;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WSTRB" *) input [3:0]S_AXI_WSTRB;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WVALID" *) input S_AXI_WVALID;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WREADY" *) output S_AXI_WREADY;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BRESP" *) output [1:0]S_AXI_BRESP;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BVALID" *) output S_AXI_BVALID;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BREADY" *) input S_AXI_BREADY;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARADDR" *) input [3:0]S_AXI_ARADDR;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARPROT" *) input [2:0]S_AXI_ARPROT;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARVALID" *) input S_AXI_ARVALID;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARREADY" *) output S_AXI_ARREADY;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) output [31:0]S_AXI_RDATA;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RRESP" *) output [1:0]S_AXI_RRESP;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RVALID" *) output S_AXI_RVALID;
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RREADY" *) input S_AXI_RREADY;

  wire S_AXI_ACLK;
  wire S_AXI_ACLK_IBUF;
  wire S_AXI_ACLK_IBUF_BUFG;
  wire [3:0]S_AXI_ARADDR;
  wire [3:2]S_AXI_ARADDR_IBUF;
  wire S_AXI_ARESETN;
  wire S_AXI_ARESETN_IBUF;
  wire S_AXI_ARREADY;
  wire S_AXI_ARREADY_OBUF;
  wire S_AXI_ARVALID;
  wire S_AXI_ARVALID_IBUF;
  wire [3:0]S_AXI_AWADDR;
  wire [3:2]S_AXI_AWADDR_IBUF;
  wire S_AXI_AWREADY;
  wire S_AXI_AWVALID;
  wire S_AXI_AWVALID_IBUF;
  wire S_AXI_BREADY;
  wire S_AXI_BREADY_IBUF;
  wire [1:0]S_AXI_BRESP;
  wire S_AXI_BVALID;
  wire S_AXI_BVALID_OBUF;
  wire [31:0]S_AXI_RDATA;
  wire [31:0]S_AXI_RDATA_OBUF;
  wire S_AXI_RREADY;
  wire S_AXI_RREADY_IBUF;
  wire [1:0]S_AXI_RRESP;
  wire S_AXI_RVALID;
  wire S_AXI_RVALID_OBUF;
  wire [31:0]S_AXI_WDATA;
  wire [31:0]S_AXI_WDATA_IBUF;
  wire S_AXI_WREADY;
  wire S_AXI_WREADY_OBUF;
  wire [3:0]S_AXI_WSTRB;
  wire [3:0]S_AXI_WSTRB_IBUF;
  wire S_AXI_WVALID;
  wire S_AXI_WVALID_IBUF;
  wire bvalid_i_1_n_0;
  wire p_0_in;
  wire [31:7]p_1_in;
  wire \rdata[0]_i_1_n_0 ;
  wire \rdata[10]_i_1_n_0 ;
  wire \rdata[11]_i_1_n_0 ;
  wire \rdata[12]_i_1_n_0 ;
  wire \rdata[13]_i_1_n_0 ;
  wire \rdata[14]_i_1_n_0 ;
  wire \rdata[15]_i_1_n_0 ;
  wire \rdata[16]_i_1_n_0 ;
  wire \rdata[17]_i_1_n_0 ;
  wire \rdata[18]_i_1_n_0 ;
  wire \rdata[19]_i_1_n_0 ;
  wire \rdata[1]_i_1_n_0 ;
  wire \rdata[20]_i_1_n_0 ;
  wire \rdata[21]_i_1_n_0 ;
  wire \rdata[22]_i_1_n_0 ;
  wire \rdata[23]_i_1_n_0 ;
  wire \rdata[24]_i_1_n_0 ;
  wire \rdata[25]_i_1_n_0 ;
  wire \rdata[26]_i_1_n_0 ;
  wire \rdata[27]_i_1_n_0 ;
  wire \rdata[28]_i_1_n_0 ;
  wire \rdata[29]_i_1_n_0 ;
  wire \rdata[2]_i_1_n_0 ;
  wire \rdata[30]_i_1_n_0 ;
  wire \rdata[31]_i_1_n_0 ;
  wire \rdata[3]_i_1_n_0 ;
  wire \rdata[4]_i_1_n_0 ;
  wire \rdata[5]_i_1_n_0 ;
  wire \rdata[6]_i_1_n_0 ;
  wire \rdata[7]_i_1_n_0 ;
  wire \rdata[8]_i_1_n_0 ;
  wire \rdata[9]_i_1_n_0 ;
  wire rvalid_i_1_n_0;
  wire [31:0]slv_reg0;

  BUFG S_AXI_ACLK_IBUF_BUFG_inst
       (.I(S_AXI_ACLK_IBUF),
        .O(S_AXI_ACLK_IBUF_BUFG));
  (* X_INTERFACE_INFO = "xilinx.com:signal:clock:1.0 S_AXI_ACLK CLK" *) 
  (* X_INTERFACE_PARAMETER = "ASSOCIATED_BUSIF S_AXI, ASSOCIATED_RESET S_AXI_ARESETN" *) 
  IBUF S_AXI_ACLK_IBUF_inst
       (.I(S_AXI_ACLK),
        .O(S_AXI_ACLK_IBUF));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARADDR" *) 
  IBUF \S_AXI_ARADDR_IBUF[2]_inst 
       (.I(S_AXI_ARADDR[2]),
        .O(S_AXI_ARADDR_IBUF[2]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARADDR" *) 
  IBUF \S_AXI_ARADDR_IBUF[3]_inst 
       (.I(S_AXI_ARADDR[3]),
        .O(S_AXI_ARADDR_IBUF[3]));
  (* X_INTERFACE_INFO = "xilinx.com:signal:reset:1.0 S_AXI_ARESETN RST" *) 
  (* X_INTERFACE_PARAMETER = "POLARITY ACTIVE_LOW" *) 
  IBUF S_AXI_ARESETN_IBUF_inst
       (.I(S_AXI_ARESETN),
        .O(S_AXI_ARESETN_IBUF));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARREADY" *) 
  OBUF S_AXI_ARREADY_OBUF_inst
       (.I(S_AXI_ARREADY_OBUF),
        .O(S_AXI_ARREADY));
  LUT2 #(
    .INIT(4'h2)) 
    S_AXI_ARREADY_OBUF_inst_i_1
       (.I0(S_AXI_ARVALID_IBUF),
        .I1(S_AXI_RVALID_OBUF),
        .O(S_AXI_ARREADY_OBUF));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI ARVALID" *) 
  IBUF S_AXI_ARVALID_IBUF_inst
       (.I(S_AXI_ARVALID),
        .O(S_AXI_ARVALID_IBUF));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWADDR" *) 
  IBUF \S_AXI_AWADDR_IBUF[2]_inst 
       (.I(S_AXI_AWADDR[2]),
        .O(S_AXI_AWADDR_IBUF[2]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWADDR" *) 
  IBUF \S_AXI_AWADDR_IBUF[3]_inst 
       (.I(S_AXI_AWADDR[3]),
        .O(S_AXI_AWADDR_IBUF[3]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWREADY" *) 
  OBUF S_AXI_AWREADY_OBUF_inst
       (.I(S_AXI_WREADY_OBUF),
        .O(S_AXI_AWREADY));
  (* SOFT_HLUTNM = "soft_lutpair0" *) 
  LUT3 #(
    .INIT(8'h20)) 
    S_AXI_AWREADY_OBUF_inst_i_1
       (.I0(S_AXI_AWVALID_IBUF),
        .I1(S_AXI_BVALID_OBUF),
        .I2(S_AXI_WVALID_IBUF),
        .O(S_AXI_WREADY_OBUF));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI AWVALID" *) 
  IBUF S_AXI_AWVALID_IBUF_inst
       (.I(S_AXI_AWVALID),
        .O(S_AXI_AWVALID_IBUF));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BREADY" *) 
  IBUF S_AXI_BREADY_IBUF_inst
       (.I(S_AXI_BREADY),
        .O(S_AXI_BREADY_IBUF));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BRESP" *) 
  OBUF \S_AXI_BRESP_OBUF[0]_inst 
       (.I(1'b0),
        .O(S_AXI_BRESP[0]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BRESP" *) 
  OBUF \S_AXI_BRESP_OBUF[1]_inst 
       (.I(1'b0),
        .O(S_AXI_BRESP[1]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BVALID" *) 
  OBUF S_AXI_BVALID_OBUF_inst
       (.I(S_AXI_BVALID_OBUF),
        .O(S_AXI_BVALID));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[0]_inst 
       (.I(S_AXI_RDATA_OBUF[0]),
        .O(S_AXI_RDATA[0]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[10]_inst 
       (.I(S_AXI_RDATA_OBUF[10]),
        .O(S_AXI_RDATA[10]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[11]_inst 
       (.I(S_AXI_RDATA_OBUF[11]),
        .O(S_AXI_RDATA[11]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[12]_inst 
       (.I(S_AXI_RDATA_OBUF[12]),
        .O(S_AXI_RDATA[12]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[13]_inst 
       (.I(S_AXI_RDATA_OBUF[13]),
        .O(S_AXI_RDATA[13]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[14]_inst 
       (.I(S_AXI_RDATA_OBUF[14]),
        .O(S_AXI_RDATA[14]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[15]_inst 
       (.I(S_AXI_RDATA_OBUF[15]),
        .O(S_AXI_RDATA[15]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[16]_inst 
       (.I(S_AXI_RDATA_OBUF[16]),
        .O(S_AXI_RDATA[16]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[17]_inst 
       (.I(S_AXI_RDATA_OBUF[17]),
        .O(S_AXI_RDATA[17]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[18]_inst 
       (.I(S_AXI_RDATA_OBUF[18]),
        .O(S_AXI_RDATA[18]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[19]_inst 
       (.I(S_AXI_RDATA_OBUF[19]),
        .O(S_AXI_RDATA[19]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[1]_inst 
       (.I(S_AXI_RDATA_OBUF[1]),
        .O(S_AXI_RDATA[1]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[20]_inst 
       (.I(S_AXI_RDATA_OBUF[20]),
        .O(S_AXI_RDATA[20]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[21]_inst 
       (.I(S_AXI_RDATA_OBUF[21]),
        .O(S_AXI_RDATA[21]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[22]_inst 
       (.I(S_AXI_RDATA_OBUF[22]),
        .O(S_AXI_RDATA[22]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[23]_inst 
       (.I(S_AXI_RDATA_OBUF[23]),
        .O(S_AXI_RDATA[23]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[24]_inst 
       (.I(S_AXI_RDATA_OBUF[24]),
        .O(S_AXI_RDATA[24]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[25]_inst 
       (.I(S_AXI_RDATA_OBUF[25]),
        .O(S_AXI_RDATA[25]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[26]_inst 
       (.I(S_AXI_RDATA_OBUF[26]),
        .O(S_AXI_RDATA[26]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[27]_inst 
       (.I(S_AXI_RDATA_OBUF[27]),
        .O(S_AXI_RDATA[27]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[28]_inst 
       (.I(S_AXI_RDATA_OBUF[28]),
        .O(S_AXI_RDATA[28]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[29]_inst 
       (.I(S_AXI_RDATA_OBUF[29]),
        .O(S_AXI_RDATA[29]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[2]_inst 
       (.I(S_AXI_RDATA_OBUF[2]),
        .O(S_AXI_RDATA[2]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[30]_inst 
       (.I(S_AXI_RDATA_OBUF[30]),
        .O(S_AXI_RDATA[30]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[31]_inst 
       (.I(S_AXI_RDATA_OBUF[31]),
        .O(S_AXI_RDATA[31]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[3]_inst 
       (.I(S_AXI_RDATA_OBUF[3]),
        .O(S_AXI_RDATA[3]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[4]_inst 
       (.I(S_AXI_RDATA_OBUF[4]),
        .O(S_AXI_RDATA[4]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[5]_inst 
       (.I(S_AXI_RDATA_OBUF[5]),
        .O(S_AXI_RDATA[5]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[6]_inst 
       (.I(S_AXI_RDATA_OBUF[6]),
        .O(S_AXI_RDATA[6]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[7]_inst 
       (.I(S_AXI_RDATA_OBUF[7]),
        .O(S_AXI_RDATA[7]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[8]_inst 
       (.I(S_AXI_RDATA_OBUF[8]),
        .O(S_AXI_RDATA[8]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  OBUF \S_AXI_RDATA_OBUF[9]_inst 
       (.I(S_AXI_RDATA_OBUF[9]),
        .O(S_AXI_RDATA[9]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RREADY" *) 
  IBUF S_AXI_RREADY_IBUF_inst
       (.I(S_AXI_RREADY),
        .O(S_AXI_RREADY_IBUF));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RRESP" *) 
  OBUF \S_AXI_RRESP_OBUF[0]_inst 
       (.I(1'b0),
        .O(S_AXI_RRESP[0]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RRESP" *) 
  OBUF \S_AXI_RRESP_OBUF[1]_inst 
       (.I(1'b0),
        .O(S_AXI_RRESP[1]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RVALID" *) 
  OBUF S_AXI_RVALID_OBUF_inst
       (.I(S_AXI_RVALID_OBUF),
        .O(S_AXI_RVALID));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[0]_inst 
       (.I(S_AXI_WDATA[0]),
        .O(S_AXI_WDATA_IBUF[0]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[10]_inst 
       (.I(S_AXI_WDATA[10]),
        .O(S_AXI_WDATA_IBUF[10]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[11]_inst 
       (.I(S_AXI_WDATA[11]),
        .O(S_AXI_WDATA_IBUF[11]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[12]_inst 
       (.I(S_AXI_WDATA[12]),
        .O(S_AXI_WDATA_IBUF[12]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[13]_inst 
       (.I(S_AXI_WDATA[13]),
        .O(S_AXI_WDATA_IBUF[13]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[14]_inst 
       (.I(S_AXI_WDATA[14]),
        .O(S_AXI_WDATA_IBUF[14]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[15]_inst 
       (.I(S_AXI_WDATA[15]),
        .O(S_AXI_WDATA_IBUF[15]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[16]_inst 
       (.I(S_AXI_WDATA[16]),
        .O(S_AXI_WDATA_IBUF[16]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[17]_inst 
       (.I(S_AXI_WDATA[17]),
        .O(S_AXI_WDATA_IBUF[17]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[18]_inst 
       (.I(S_AXI_WDATA[18]),
        .O(S_AXI_WDATA_IBUF[18]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[19]_inst 
       (.I(S_AXI_WDATA[19]),
        .O(S_AXI_WDATA_IBUF[19]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[1]_inst 
       (.I(S_AXI_WDATA[1]),
        .O(S_AXI_WDATA_IBUF[1]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[20]_inst 
       (.I(S_AXI_WDATA[20]),
        .O(S_AXI_WDATA_IBUF[20]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[21]_inst 
       (.I(S_AXI_WDATA[21]),
        .O(S_AXI_WDATA_IBUF[21]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[22]_inst 
       (.I(S_AXI_WDATA[22]),
        .O(S_AXI_WDATA_IBUF[22]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[23]_inst 
       (.I(S_AXI_WDATA[23]),
        .O(S_AXI_WDATA_IBUF[23]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[24]_inst 
       (.I(S_AXI_WDATA[24]),
        .O(S_AXI_WDATA_IBUF[24]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[25]_inst 
       (.I(S_AXI_WDATA[25]),
        .O(S_AXI_WDATA_IBUF[25]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[26]_inst 
       (.I(S_AXI_WDATA[26]),
        .O(S_AXI_WDATA_IBUF[26]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[27]_inst 
       (.I(S_AXI_WDATA[27]),
        .O(S_AXI_WDATA_IBUF[27]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[28]_inst 
       (.I(S_AXI_WDATA[28]),
        .O(S_AXI_WDATA_IBUF[28]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[29]_inst 
       (.I(S_AXI_WDATA[29]),
        .O(S_AXI_WDATA_IBUF[29]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[2]_inst 
       (.I(S_AXI_WDATA[2]),
        .O(S_AXI_WDATA_IBUF[2]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[30]_inst 
       (.I(S_AXI_WDATA[30]),
        .O(S_AXI_WDATA_IBUF[30]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[31]_inst 
       (.I(S_AXI_WDATA[31]),
        .O(S_AXI_WDATA_IBUF[31]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[3]_inst 
       (.I(S_AXI_WDATA[3]),
        .O(S_AXI_WDATA_IBUF[3]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[4]_inst 
       (.I(S_AXI_WDATA[4]),
        .O(S_AXI_WDATA_IBUF[4]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[5]_inst 
       (.I(S_AXI_WDATA[5]),
        .O(S_AXI_WDATA_IBUF[5]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[6]_inst 
       (.I(S_AXI_WDATA[6]),
        .O(S_AXI_WDATA_IBUF[6]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[7]_inst 
       (.I(S_AXI_WDATA[7]),
        .O(S_AXI_WDATA_IBUF[7]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[8]_inst 
       (.I(S_AXI_WDATA[8]),
        .O(S_AXI_WDATA_IBUF[8]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WDATA" *) 
  IBUF \S_AXI_WDATA_IBUF[9]_inst 
       (.I(S_AXI_WDATA[9]),
        .O(S_AXI_WDATA_IBUF[9]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WREADY" *) 
  OBUF S_AXI_WREADY_OBUF_inst
       (.I(S_AXI_WREADY_OBUF),
        .O(S_AXI_WREADY));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WSTRB" *) 
  IBUF \S_AXI_WSTRB_IBUF[0]_inst 
       (.I(S_AXI_WSTRB[0]),
        .O(S_AXI_WSTRB_IBUF[0]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WSTRB" *) 
  IBUF \S_AXI_WSTRB_IBUF[1]_inst 
       (.I(S_AXI_WSTRB[1]),
        .O(S_AXI_WSTRB_IBUF[1]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WSTRB" *) 
  IBUF \S_AXI_WSTRB_IBUF[2]_inst 
       (.I(S_AXI_WSTRB[2]),
        .O(S_AXI_WSTRB_IBUF[2]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WSTRB" *) 
  IBUF \S_AXI_WSTRB_IBUF[3]_inst 
       (.I(S_AXI_WSTRB[3]),
        .O(S_AXI_WSTRB_IBUF[3]));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI WVALID" *) 
  IBUF S_AXI_WVALID_IBUF_inst
       (.I(S_AXI_WVALID),
        .O(S_AXI_WVALID_IBUF));
  (* SOFT_HLUTNM = "soft_lutpair0" *) 
  LUT5 #(
    .INIT(32'h20EC0000)) 
    bvalid_i_1
       (.I0(S_AXI_AWVALID_IBUF),
        .I1(S_AXI_BVALID_OBUF),
        .I2(S_AXI_WVALID_IBUF),
        .I3(S_AXI_BREADY_IBUF),
        .I4(S_AXI_ARESETN_IBUF),
        .O(bvalid_i_1_n_0));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI BVALID" *) 
  FDRE #(
    .INIT(1'b0)) 
    bvalid_reg
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(1'b1),
        .D(bvalid_i_1_n_0),
        .Q(S_AXI_BVALID_OBUF),
        .R(1'b0));
  (* SOFT_HLUTNM = "soft_lutpair16" *) 
  LUT3 #(
    .INIT(8'h04)) 
    \rdata[0]_i_1 
       (.I0(S_AXI_ARADDR_IBUF[2]),
        .I1(slv_reg0[0]),
        .I2(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[0]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair5" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[10]_i_1 
       (.I0(slv_reg0[10]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[9]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[10]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair6" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[11]_i_1 
       (.I0(slv_reg0[11]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[10]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[11]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair6" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[12]_i_1 
       (.I0(slv_reg0[12]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[11]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[12]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair7" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[13]_i_1 
       (.I0(slv_reg0[13]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[12]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[13]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair7" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[14]_i_1 
       (.I0(slv_reg0[14]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[13]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[14]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair8" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[15]_i_1 
       (.I0(slv_reg0[15]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[14]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[15]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair8" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[16]_i_1 
       (.I0(slv_reg0[16]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[15]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[16]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair9" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[17]_i_1 
       (.I0(slv_reg0[17]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[16]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[17]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair9" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[18]_i_1 
       (.I0(slv_reg0[18]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[17]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[18]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair10" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[19]_i_1 
       (.I0(slv_reg0[19]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[18]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[19]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair1" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[1]_i_1 
       (.I0(slv_reg0[1]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[0]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[1]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair10" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[20]_i_1 
       (.I0(slv_reg0[20]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[19]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[20]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair11" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[21]_i_1 
       (.I0(slv_reg0[21]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[20]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[21]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair11" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[22]_i_1 
       (.I0(slv_reg0[22]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[21]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[22]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair12" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[23]_i_1 
       (.I0(slv_reg0[23]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[22]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[23]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair12" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[24]_i_1 
       (.I0(slv_reg0[24]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[23]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[24]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair13" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[25]_i_1 
       (.I0(slv_reg0[25]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[24]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[25]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair13" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[26]_i_1 
       (.I0(slv_reg0[26]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[25]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[26]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair14" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[27]_i_1 
       (.I0(slv_reg0[27]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[26]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[27]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair14" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[28]_i_1 
       (.I0(slv_reg0[28]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[27]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[28]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair15" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[29]_i_1 
       (.I0(slv_reg0[29]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[28]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[29]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair1" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[2]_i_1 
       (.I0(slv_reg0[2]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[1]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[2]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair15" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[30]_i_1 
       (.I0(slv_reg0[30]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[29]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[30]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair16" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[31]_i_1 
       (.I0(slv_reg0[31]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[30]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[31]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair2" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[3]_i_1 
       (.I0(slv_reg0[3]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[2]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[3]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair2" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[4]_i_1 
       (.I0(slv_reg0[4]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[3]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[4]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair3" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[5]_i_1 
       (.I0(slv_reg0[5]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[4]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[5]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair3" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[6]_i_1 
       (.I0(slv_reg0[6]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[5]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[6]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair4" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[7]_i_1 
       (.I0(slv_reg0[7]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[6]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[7]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair4" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[8]_i_1 
       (.I0(slv_reg0[8]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[7]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[8]_i_1_n_0 ));
  (* SOFT_HLUTNM = "soft_lutpair5" *) 
  LUT4 #(
    .INIT(16'h00E2)) 
    \rdata[9]_i_1 
       (.I0(slv_reg0[9]),
        .I1(S_AXI_ARADDR_IBUF[2]),
        .I2(slv_reg0[8]),
        .I3(S_AXI_ARADDR_IBUF[3]),
        .O(\rdata[9]_i_1_n_0 ));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[0] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[0]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[0]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[10] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[10]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[10]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[11] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[11]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[11]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[12] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[12]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[12]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[13] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[13]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[13]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[14] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[14]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[14]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[15] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[15]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[15]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[16] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[16]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[16]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[17] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[17]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[17]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[18] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[18]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[18]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[19] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[19]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[19]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[1] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[1]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[1]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[20] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[20]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[20]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[21] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[21]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[21]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[22] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[22]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[22]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[23] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[23]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[23]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[24] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[24]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[24]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[25] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[25]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[25]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[26] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[26]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[26]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[27] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[27]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[27]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[28] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[28]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[28]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[29] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[29]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[29]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[2] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[2]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[2]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[30] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[30]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[30]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[31] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[31]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[31]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[3] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[3]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[3]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[4] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[4]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[4]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[5] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[5]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[5]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[6] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[6]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[6]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[7] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[7]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[7]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[8] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[8]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[8]),
        .R(p_0_in));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RDATA" *) 
  FDRE #(
    .INIT(1'b0)) 
    \rdata_reg[9] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(S_AXI_ARREADY_OBUF),
        .D(\rdata[9]_i_1_n_0 ),
        .Q(S_AXI_RDATA_OBUF[9]),
        .R(p_0_in));
  LUT4 #(
    .INIT(16'h2E00)) 
    rvalid_i_1
       (.I0(S_AXI_ARVALID_IBUF),
        .I1(S_AXI_RVALID_OBUF),
        .I2(S_AXI_RREADY_IBUF),
        .I3(S_AXI_ARESETN_IBUF),
        .O(rvalid_i_1_n_0));
  (* X_INTERFACE_INFO = "xilinx.com:interface:aximm:1.0 S_AXI RVALID" *) 
  FDRE #(
    .INIT(1'b0)) 
    rvalid_reg
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(1'b1),
        .D(rvalid_i_1_n_0),
        .Q(S_AXI_RVALID_OBUF),
        .R(1'b0));
  LUT6 #(
    .INIT(64'h0000002000000000)) 
    \slv_reg0[15]_i_1 
       (.I0(S_AXI_WVALID_IBUF),
        .I1(S_AXI_BVALID_OBUF),
        .I2(S_AXI_AWVALID_IBUF),
        .I3(S_AXI_AWADDR_IBUF[3]),
        .I4(S_AXI_AWADDR_IBUF[2]),
        .I5(S_AXI_WSTRB_IBUF[1]),
        .O(p_1_in[15]));
  LUT6 #(
    .INIT(64'h0000002000000000)) 
    \slv_reg0[23]_i_1 
       (.I0(S_AXI_WVALID_IBUF),
        .I1(S_AXI_BVALID_OBUF),
        .I2(S_AXI_AWVALID_IBUF),
        .I3(S_AXI_AWADDR_IBUF[3]),
        .I4(S_AXI_AWADDR_IBUF[2]),
        .I5(S_AXI_WSTRB_IBUF[2]),
        .O(p_1_in[23]));
  LUT1 #(
    .INIT(2'h1)) 
    \slv_reg0[31]_i_1 
       (.I0(S_AXI_ARESETN_IBUF),
        .O(p_0_in));
  LUT6 #(
    .INIT(64'h0000002000000000)) 
    \slv_reg0[31]_i_2 
       (.I0(S_AXI_WVALID_IBUF),
        .I1(S_AXI_BVALID_OBUF),
        .I2(S_AXI_AWVALID_IBUF),
        .I3(S_AXI_AWADDR_IBUF[3]),
        .I4(S_AXI_AWADDR_IBUF[2]),
        .I5(S_AXI_WSTRB_IBUF[3]),
        .O(p_1_in[31]));
  LUT6 #(
    .INIT(64'h0000002000000000)) 
    \slv_reg0[7]_i_1 
       (.I0(S_AXI_WVALID_IBUF),
        .I1(S_AXI_BVALID_OBUF),
        .I2(S_AXI_AWVALID_IBUF),
        .I3(S_AXI_AWADDR_IBUF[3]),
        .I4(S_AXI_AWADDR_IBUF[2]),
        .I5(S_AXI_WSTRB_IBUF[0]),
        .O(p_1_in[7]));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[0] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[7]),
        .D(S_AXI_WDATA_IBUF[0]),
        .Q(slv_reg0[0]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[10] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[15]),
        .D(S_AXI_WDATA_IBUF[10]),
        .Q(slv_reg0[10]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[11] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[15]),
        .D(S_AXI_WDATA_IBUF[11]),
        .Q(slv_reg0[11]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[12] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[15]),
        .D(S_AXI_WDATA_IBUF[12]),
        .Q(slv_reg0[12]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[13] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[15]),
        .D(S_AXI_WDATA_IBUF[13]),
        .Q(slv_reg0[13]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[14] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[15]),
        .D(S_AXI_WDATA_IBUF[14]),
        .Q(slv_reg0[14]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[15] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[15]),
        .D(S_AXI_WDATA_IBUF[15]),
        .Q(slv_reg0[15]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[16] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[23]),
        .D(S_AXI_WDATA_IBUF[16]),
        .Q(slv_reg0[16]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[17] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[23]),
        .D(S_AXI_WDATA_IBUF[17]),
        .Q(slv_reg0[17]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[18] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[23]),
        .D(S_AXI_WDATA_IBUF[18]),
        .Q(slv_reg0[18]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[19] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[23]),
        .D(S_AXI_WDATA_IBUF[19]),
        .Q(slv_reg0[19]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[1] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[7]),
        .D(S_AXI_WDATA_IBUF[1]),
        .Q(slv_reg0[1]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[20] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[23]),
        .D(S_AXI_WDATA_IBUF[20]),
        .Q(slv_reg0[20]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[21] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[23]),
        .D(S_AXI_WDATA_IBUF[21]),
        .Q(slv_reg0[21]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[22] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[23]),
        .D(S_AXI_WDATA_IBUF[22]),
        .Q(slv_reg0[22]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[23] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[23]),
        .D(S_AXI_WDATA_IBUF[23]),
        .Q(slv_reg0[23]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[24] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[31]),
        .D(S_AXI_WDATA_IBUF[24]),
        .Q(slv_reg0[24]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[25] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[31]),
        .D(S_AXI_WDATA_IBUF[25]),
        .Q(slv_reg0[25]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[26] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[31]),
        .D(S_AXI_WDATA_IBUF[26]),
        .Q(slv_reg0[26]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[27] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[31]),
        .D(S_AXI_WDATA_IBUF[27]),
        .Q(slv_reg0[27]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[28] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[31]),
        .D(S_AXI_WDATA_IBUF[28]),
        .Q(slv_reg0[28]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[29] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[31]),
        .D(S_AXI_WDATA_IBUF[29]),
        .Q(slv_reg0[29]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[2] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[7]),
        .D(S_AXI_WDATA_IBUF[2]),
        .Q(slv_reg0[2]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[30] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[31]),
        .D(S_AXI_WDATA_IBUF[30]),
        .Q(slv_reg0[30]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[31] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[31]),
        .D(S_AXI_WDATA_IBUF[31]),
        .Q(slv_reg0[31]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[3] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[7]),
        .D(S_AXI_WDATA_IBUF[3]),
        .Q(slv_reg0[3]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[4] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[7]),
        .D(S_AXI_WDATA_IBUF[4]),
        .Q(slv_reg0[4]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[5] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[7]),
        .D(S_AXI_WDATA_IBUF[5]),
        .Q(slv_reg0[5]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[6] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[7]),
        .D(S_AXI_WDATA_IBUF[6]),
        .Q(slv_reg0[6]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[7] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[7]),
        .D(S_AXI_WDATA_IBUF[7]),
        .Q(slv_reg0[7]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[8] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[15]),
        .D(S_AXI_WDATA_IBUF[8]),
        .Q(slv_reg0[8]),
        .R(p_0_in));
  FDRE #(
    .INIT(1'b0)) 
    \slv_reg0_reg[9] 
       (.C(S_AXI_ACLK_IBUF_BUFG),
        .CE(p_1_in[15]),
        .D(S_AXI_WDATA_IBUF[9]),
        .Q(slv_reg0[9]),
        .R(p_0_in));
endmodule
`ifndef GLBL
`define GLBL
`timescale  1 ps / 1 ps

module glbl ();

    parameter ROC_WIDTH = 100000;
    parameter TOC_WIDTH = 0;
    parameter GRES_WIDTH = 10000;
    parameter GRES_START = 10000;

//--------   STARTUP Globals --------------
    wire GSR;
    wire GTS;
    wire GWE;
    wire PRLD;
    wire GRESTORE;
    tri1 p_up_tmp;
    tri (weak1, strong0) PLL_LOCKG = p_up_tmp;

    wire PROGB_GLBL;
    wire CCLKO_GLBL;
    wire FCSBO_GLBL;
    wire [3:0] DO_GLBL;
    wire [3:0] DI_GLBL;
   
    reg GSR_int;
    reg GTS_int;
    reg PRLD_int;
    reg GRESTORE_int;

//--------   JTAG Globals --------------
    wire JTAG_TDO_GLBL;
    wire JTAG_TCK_GLBL;
    wire JTAG_TDI_GLBL;
    wire JTAG_TMS_GLBL;
    wire JTAG_TRST_GLBL;

    reg JTAG_CAPTURE_GLBL;
    reg JTAG_RESET_GLBL;
    reg JTAG_SHIFT_GLBL;
    reg JTAG_UPDATE_GLBL;
    reg JTAG_RUNTEST_GLBL;

    reg JTAG_SEL1_GLBL = 0;
    reg JTAG_SEL2_GLBL = 0 ;
    reg JTAG_SEL3_GLBL = 0;
    reg JTAG_SEL4_GLBL = 0;

    reg JTAG_USER_TDO1_GLBL = 1'bz;
    reg JTAG_USER_TDO2_GLBL = 1'bz;
    reg JTAG_USER_TDO3_GLBL = 1'bz;
    reg JTAG_USER_TDO4_GLBL = 1'bz;

    assign (strong1, weak0) GSR = GSR_int;
    assign (strong1, weak0) GTS = GTS_int;
    assign (weak1, weak0) PRLD = PRLD_int;
    assign (strong1, weak0) GRESTORE = GRESTORE_int;

    initial begin
	GSR_int = 1'b1;
	PRLD_int = 1'b1;
	#(ROC_WIDTH)
	GSR_int = 1'b0;
	PRLD_int = 1'b0;
    end

    initial begin
	GTS_int = 1'b1;
	#(TOC_WIDTH)
	GTS_int = 1'b0;
    end

    initial begin 
	GRESTORE_int = 1'b0;
	#(GRES_START);
	GRESTORE_int = 1'b1;
	#(GRES_WIDTH);
	GRESTORE_int = 1'b0;
    end

endmodule
`endif
