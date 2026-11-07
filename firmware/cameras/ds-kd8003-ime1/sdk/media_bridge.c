/* DS-KD8003 media bridge over the HiSilicon MPP V1 libraries loaded at run time from the runtime
 * directory (libnative_media_group.so groups them); the pipeline attributes are qualified_profile
 * below. Video: legacy VB, LVDS receiver, VI, the stock AR0237 ISP (registered through
 * libe3_isp_access with the hooks hicore supplies), VPSS, VENC H264 main + sub. Audio: AI0/AO0
 * 8 kHz, 40 ms frames, one ReleaseFrame per GetFrame; the DAC and amplifier through the codec and
 * DDM sound nodes. VB pool-info ioctl translation and the pthreadSpawn helpers come from
 * libloader_compat.so (NEEDED before libc). Teardown is a reboot: see gm_stop. */
#include "hisi_v3_types.h"
typedef unsigned long usize;typedef unsigned long long u64;
#define ARM_SAFE_READ (32768|2048|524288)
extern void *dlopen(const char *,int),*dlsym(void *,const char *);extern char *dlerror(void);
extern int open(const char *,int,...),close(int),ioctl(int,unsigned long,...),snprintf(char *,usize,const char *,...),strcmp(const char *,const char *),usleep(unsigned);
extern long read(int,void *,usize),write(int,const void *,usize);
extern void *memset(void *,int,usize),*memcpy(void *,const void *,usize);
extern int *__errno_location(void);
extern int pthread_create(unsigned long *,const void *,void *(*)(void *),void *),pthread_join(unsigned long,void **);
struct legacy_pool {unsigned block_bytes,block_count;unsigned char name[16];};
struct legacy_vb {unsigned max_pool_count;struct legacy_pool pools[16];};
struct profile {unsigned magic,version,bytes,alignment;v3_vi_dev vi;v3_vi_chn vi_channel;v3_vpss_grp vpss;v3_vpss_chn vpss_channel;v3_vpss_mode vpss_mode;v3_venc_chn encoder;struct legacy_vb vb;v3_isp_dev isp;v3_snr_dev receiver;};
_Static_assert(sizeof(struct profile)==1008,"qualified profile extent");
/* The pipeline attributes the stock application uses on this model (AR0237 over HiSPi, 1920x1080
 * H.264), field for field. Every DS-KD8003-IME1 of the supported build uses the same values. */
static const struct profile qualified_profile={
    .magic = 0x47505246u,
    .version = 1u,
    .bytes = 1008u,
    .alignment = 64u,
    .vi.intf = 5,
    .vi.cmpntMask[0] = 0xfff00000u,
    .vi.progressiveOn = 1,
    .vi.adChn[0] = -1,
    .vi.adChn[1] = -1,
    .vi.adChn[2] = -1,
    .vi.adChn[3] = -1,
    .vi.seq = 4,
    .vi.dataPath = 1,
    .vi.rgbModeOn = 1,
    .vi.rect.width = 1920u,
    .vi.rect.height = 1080u,
    .vi.bayerSize.width = 1920u,
    .vi.bayerSize.height = 1080u,
    .vi_channel.capt.width = 1920u,
    .vi_channel.capt.height = 1080u,
    .vi_channel.dest.width = 1920u,
    .vi_channel.dest.height = 1080u,
    .vi_channel.field = 2,
    .vi_channel.pixFmt = 23,
    .vi_channel.srcFps = -1,
    .vi_channel.dstFps = -1,
    .vpss.dest.width = 1920u,
    .vpss.dest.height = 1080u,
    .vpss.pixFmt = 23,
    .vpss.nredOn = 1,
    .vpss.interlMode = 1,
    .vpss_channel.srcFps = -1,
    .vpss_channel.dstFps = -1,
    .vpss_mode.userModeOn = 1,
    .vpss_mode.dest.width = 1920u,
    .vpss_mode.dest.height = 1080u,
    .vpss_mode.pixFmt = 23,
    .encoder.attrib.codec = 96,
    .encoder.attrib.h264.maxPic.width = 1920u,
    .encoder.attrib.h264.maxPic.height = 1080u,
    .encoder.attrib.h264.bufSize = 0x001fa400u,
    .encoder.attrib.h264.byFrame = 1,
    .encoder.attrib.h264.pic.width = 1920u,
    .encoder.attrib.h264.pic.height = 1080u,
    .encoder.rate.mode = 2,
    .encoder.rate.h264Vbr.gop = 30u,
    .encoder.rate.h264Vbr.statTime = 10u,
    .encoder.rate.h264Vbr.srcFps = 30u,
    .encoder.rate.h264Vbr.dstFps = 30u,
    .encoder.rate.h264Vbr.maxBitrate = 4096u,
    .encoder.rate.h264Vbr.maxQual = 50u,
    .encoder.rate.h264Vbr.minQual = 10u,
    .encoder.rate.h264Vbr.minIQual = 10u,
    .encoder.gop.normalP.ipQualDelta = -1,
    .vb.max_pool_count = 256u,
    .vb.pools[0].block_bytes = 0x00308400u,
    .vb.pools[0].block_count = 9u,
    .vb.pools[1].block_bytes = 0x0016f800u,
    .vb.pools[1].block_count = 5u,
    .vb.pools[2].block_bytes = 0x00097e00u,
    .vb.pools[2].block_count = 4u,
    .isp.capt.width = 1920u,
    .isp.capt.height = 1080u,
    .isp.framerate = 30.0f,
    .isp.bayer = 1,
    .receiver.input = 3,
    .receiver.lvds.dest.width = 1920u,
    .receiver.lvds.dest.height = 1080u,
    .receiver.lvds.prec = 2,
    .receiver.lvds.vsync.hBlank1 = 65535u,
    .receiver.lvds.vsync.hBlank2 = 65535u,
    .receiver.lvds.fid.outputFil = 1u,
    .receiver.lvds.laneId[1] = 1,
    .receiver.lvds.laneId[2] = 2,
    .receiver.lvds.laneId[3] = 3,
    .receiver.lvds.syncCode[0] = 3u,
    .receiver.lvds.syncCode[1] = 7u,
    .receiver.lvds.syncCode[2] = 1u,
    .receiver.lvds.syncCode[3] = 5u,
    .receiver.lvds.syncCode[4] = 3u,
    .receiver.lvds.syncCode[5] = 7u,
    .receiver.lvds.syncCode[6] = 1u,
    .receiver.lvds.syncCode[7] = 5u,
    .receiver.lvds.syncCode[8] = 3u,
    .receiver.lvds.syncCode[9] = 7u,
    .receiver.lvds.syncCode[10] = 1u,
    .receiver.lvds.syncCode[11] = 5u,
    .receiver.lvds.syncCode[12] = 3u,
    .receiver.lvds.syncCode[13] = 7u,
    .receiver.lvds.syncCode[14] = 1u,
    .receiver.lvds.syncCode[15] = 5u,
    .receiver.lvds.syncCode[16] = 3u,
    .receiver.lvds.syncCode[17] = 7u,
    .receiver.lvds.syncCode[18] = 1u,
    .receiver.lvds.syncCode[19] = 5u,
    .receiver.lvds.syncCode[20] = 3u,
    .receiver.lvds.syncCode[21] = 7u,
    .receiver.lvds.syncCode[22] = 1u,
    .receiver.lvds.syncCode[23] = 5u,
    .receiver.lvds.syncCode[24] = 3u,
    .receiver.lvds.syncCode[25] = 7u,
    .receiver.lvds.syncCode[26] = 1u,
    .receiver.lvds.syncCode[27] = 5u,
    .receiver.lvds.syncCode[28] = 3u,
    .receiver.lvds.syncCode[29] = 7u,
    .receiver.lvds.syncCode[30] = 1u,
    .receiver.lvds.syncCode[31] = 5u,
    .receiver.lvds.syncCode[32] = 3u,
    .receiver.lvds.syncCode[33] = 7u,
    .receiver.lvds.syncCode[34] = 1u,
    .receiver.lvds.syncCode[35] = 5u,
    .receiver.lvds.syncCode[36] = 3u,
    .receiver.lvds.syncCode[37] = 7u,
    .receiver.lvds.syncCode[38] = 1u,
    .receiver.lvds.syncCode[39] = 5u,
    .receiver.lvds.syncCode[40] = 3u,
    .receiver.lvds.syncCode[41] = 7u,
    .receiver.lvds.syncCode[42] = 1u,
    .receiver.lvds.syncCode[43] = 5u,
    .receiver.lvds.syncCode[44] = 3u,
    .receiver.lvds.syncCode[45] = 7u,
    .receiver.lvds.syncCode[46] = 1u,
    .receiver.lvds.syncCode[47] = 5u,
    .receiver.lvds.syncCode[48] = 3u,
    .receiver.lvds.syncCode[49] = 7u,
    .receiver.lvds.syncCode[50] = 1u,
    .receiver.lvds.syncCode[51] = 5u,
    .receiver.lvds.syncCode[52] = 3u,
    .receiver.lvds.syncCode[53] = 7u,
    .receiver.lvds.syncCode[54] = 1u,
    .receiver.lvds.syncCode[55] = 5u,
    .receiver.lvds.syncCode[56] = 3u,
    .receiver.lvds.syncCode[57] = 7u,
    .receiver.lvds.syncCode[58] = 1u,
    .receiver.lvds.syncCode[59] = 5u,
    .receiver.lvds.syncCode[60] = 3u,
    .receiver.lvds.syncCode[61] = 7u,
    .receiver.lvds.syncCode[62] = 1u,
    .receiver.lvds.syncCode[63] = 5u,
};
_Static_assert(sizeof(v3_venc_pack)==136&&sizeof(v3_venc_strm)==376,"VENC lease ABI");
struct aio_attr {unsigned rate,width,mode,sound,ex,frames,points,channels,clock;};
struct audio_frame {unsigned width,sound;unsigned virt[2],phys[2];u64 timestamp;unsigned sequence,length,pool[2];};
struct aec_frame {struct audio_frame reference;unsigned valid,bind;};
_Static_assert(sizeof(struct aio_attr)==36&&sizeof(struct audio_frame)==48&&sizeof(struct aec_frame)==56,"audio ABI");
static struct {
 int(*sys_exit)(void),(*sys_init)(void),(*sys_set)(unsigned *),(*vb_exit)(void),(*vb_init)(void);
 int(*vi_dev_set)(int,v3_vi_dev *),(*vi_dev_enable)(int),(*vi_dev_disable)(int),(*vi_chn_set)(int,v3_vi_chn *),(*vi_chn_enable)(int),(*vi_chn_disable)(int);
 int(*isp_type)(int),(*sensor_register)(int),(*sensor_unregister)(int),(*isp_mem)(int),(*isp_wdr)(int,unsigned *),(*isp_pub)(int,v3_isp_dev *),(*isp_init)(int),(*isp_run)(int),(*isp_exit)(int);
 int(*vpss_create)(int,v3_vpss_grp *),(*vpss_start)(int),(*vpss_stop)(int),(*vpss_destroy)(int),(*vpss_set)(int,int,v3_vpss_chn *),(*vpss_mode)(int,int,v3_vpss_mode *),(*vpss_enable)(int,int),(*vpss_disable)(int,int);
 int(*bind)(v3_sys_bind *,v3_sys_bind *),(*unbind)(v3_sys_bind *,v3_sys_bind *);
 int(*venc_create)(int,v3_venc_chn *),(*venc_start)(int),(*venc_stop)(int),(*venc_destroy)(int),(*venc_query)(int,v3_venc_stat *),(*venc_get)(int,v3_venc_strm *,unsigned),(*venc_release)(int,v3_venc_strm *),(*venc_getfd)(int);
 int(*ai_set)(int,const struct aio_attr *),(*ai_enable)(int),(*ai_disable)(int),(*ai_chn_enable)(int,int),(*ai_chn_disable)(int,int),(*ai_get_param)(int,int,unsigned *),(*ai_set_param)(int,int,const unsigned *);
 int(*sensor_read)(unsigned,unsigned *),(*sensor_write)(unsigned,unsigned);
 unsigned *capture_mode;void **isp_inner;int(*isp_saturation)(void *,int);int(*isp_set_key)(void *,int,unsigned *);
 int(*isp_preinit)(int,int,int),(*isp_register_cb)(void *),(*isp_get_mem)(void *),(*isp_create)(void *,void *,int);
 int(*ai_get)(int,int,struct audio_frame *,struct aec_frame *,int),(*ai_release)(int,int,const struct audio_frame *,const struct aec_frame *);
 int(*ao_set)(int,const struct aio_attr *),(*ao_enable)(int),(*ao_chn_enable)(int,int),(*ao_send)(int,int,const struct audio_frame *,int),(*ao_disable)(int),(*ao_chn_disable)(int,int);
 int(*isp_fn[9])(void *,int);
} a;
static struct profile p;
static int vb,sys,vd,vc,sensor,isp,thread,group,started,channel,vi_link,venc,enc_link,receive,ai_dev,ai_chn;
static unsigned long tid;static int uncertain;
static v3_sys_bind source_vi={V3_SYS_MOD_VIU,0,0},dest_vpss={V3_SYS_MOD_VPSS,0,0},source_vpss={V3_SYS_MOD_VPSS,0,0},dest_venc={V3_SYS_MOD_VENC,0,0};
static int write_all(int fd,const void *raw,usize size){const unsigned char *q=raw;while(size){long n=write(fd,q,size);if(n<=0||(usize)n>size)return -1;q+=n;size-=(usize)n;}return 0;}
void gm_message(const char *step,int rc){int e=rc?*__errno_location():0;char b[176];int n=snprintf(b,sizeof b,"\n{\"kind\":\"media-core\",\"step\":\"%s\",\"status\":%d,\"errno\":%d}\n",step,rc,e);if(n>0&&(usize)n<sizeof b)write_all(1,b,(usize)n);}
static int load(const char *d){char q[192];snprintf(q,sizeof q,"%s/libnative_media_group.so",d);void *h=dlopen(q,2|256);if(!h)return -1;
#define RESOLVE(field,name) do{(void)dlerror();*(void **)(&a.field)=dlsym(h,name);if(!a.field||dlerror())return -1;}while(0)
 RESOLVE(sys_exit,"HI_MPI_SYS_Exit");RESOLVE(sys_init,"HI_MPI_SYS_Init");RESOLVE(sys_set,"HI_MPI_SYS_SetConf");RESOLVE(vb_exit,"HI_MPI_VB_Exit");RESOLVE(vb_init,"HI_MPI_VB_Init");
 RESOLVE(vi_dev_set,"HI_MPI_VI_SetDevAttr");RESOLVE(vi_dev_enable,"HI_MPI_VI_EnableDev");RESOLVE(vi_dev_disable,"HI_MPI_VI_DisableDev");RESOLVE(vi_chn_set,"HI_MPI_VI_SetChnAttr");RESOLVE(vi_chn_enable,"HI_MPI_VI_EnableChn");RESOLVE(vi_chn_disable,"HI_MPI_VI_DisableChn");
 RESOLVE(isp_type,"ISP_Set_Sensor_Type");RESOLVE(sensor_register,"sensor_register_callback_ar0237");RESOLVE(sensor_unregister,"sensor_unregister_callback_ar0237");RESOLVE(isp_mem,"HI_MPI_ISP_MemInit");RESOLVE(isp_wdr,"HI_MPI_ISP_SetWDRMode");RESOLVE(isp_pub,"HI_MPI_ISP_SetPubAttr");RESOLVE(isp_init,"HI_MPI_ISP_Init");RESOLVE(isp_run,"HI_MPI_ISP_Run");RESOLVE(isp_exit,"HI_MPI_ISP_Exit");
 RESOLVE(vpss_create,"HI_MPI_VPSS_CreateGrp");RESOLVE(vpss_start,"HI_MPI_VPSS_StartGrp");RESOLVE(vpss_stop,"HI_MPI_VPSS_StopGrp");RESOLVE(vpss_destroy,"HI_MPI_VPSS_DestroyGrp");RESOLVE(vpss_set,"HI_MPI_VPSS_SetChnAttr");RESOLVE(vpss_mode,"HI_MPI_VPSS_SetChnMode");RESOLVE(vpss_enable,"HI_MPI_VPSS_EnableChn");RESOLVE(vpss_disable,"HI_MPI_VPSS_DisableChn");
 RESOLVE(bind,"HI_MPI_SYS_Bind");RESOLVE(unbind,"HI_MPI_SYS_UnBind");RESOLVE(venc_create,"HI_MPI_VENC_CreateChn");RESOLVE(venc_start,"HI_MPI_VENC_StartRecvPic");RESOLVE(venc_stop,"HI_MPI_VENC_StopRecvPic");RESOLVE(venc_destroy,"HI_MPI_VENC_DestroyChn");RESOLVE(venc_query,"HI_MPI_VENC_Query");RESOLVE(venc_get,"HI_MPI_VENC_GetStream");RESOLVE(venc_release,"HI_MPI_VENC_ReleaseStream");
 *(void **)(&a.venc_getfd)=dlsym(h,"HI_MPI_VENC_GetFd"); /* optional: without it the caller polls */
 RESOLVE(capture_mode,"Capture_Mode");RESOLVE(isp_inner,"isplib_inner_param");RESOLVE(isp_saturation,"ISP_Set_Saturation");RESOLVE(isp_set_key,"ISP_SetKeyParam");RESOLVE(isp_preinit,"ISP_PreInit");RESOLVE(isp_register_cb,"ISP_Register_CallBackFunc");RESOLVE(isp_get_mem,"ISP_GetMemSize");RESOLVE(isp_create,"ISP_Create");
 RESOLVE(ao_set,"HI_MPI_AO_SetPubAttr");RESOLVE(ao_enable,"HI_MPI_AO_Enable");RESOLVE(ao_chn_enable,"HI_MPI_AO_EnableChn");RESOLVE(ao_send,"HI_MPI_AO_SendFrame");RESOLVE(ao_disable,"HI_MPI_AO_Disable");RESOLVE(ao_chn_disable,"HI_MPI_AO_DisableChn");
 RESOLVE(isp_fn[0],"ISP_Set_Brightness");RESOLVE(isp_fn[1],"ISP_Set_Contrast");RESOLVE(isp_fn[2],"ISP_Set_Saturation");RESOLVE(isp_fn[3],"ISP_Set_Sharpeness");RESOLVE(isp_fn[4],"ISP_Set_Spatial_Denoise_Strength");RESOLVE(isp_fn[5],"ISP_Set_Anti_Fog_En");RESOLVE(isp_fn[6],"ISP_Set_Anti_Fog_Strength");RESOLVE(isp_fn[7],"ISP_Set_Anti_Flicker_Mode");RESOLVE(isp_fn[8],"ISP_Set_Awb_Mode");
 RESOLVE(sensor_read,"ISP_Sensor_Reg_Read");RESOLVE(sensor_write,"ISP_Sensor_Reg_Write");
 RESOLVE(ai_set,"HI_MPI_AI_SetPubAttr");RESOLVE(ai_enable,"HI_MPI_AI_Enable");RESOLVE(ai_disable,"HI_MPI_AI_Disable");RESOLVE(ai_chn_enable,"HI_MPI_AI_EnableChn");RESOLVE(ai_chn_disable,"HI_MPI_AI_DisableChn");RESOLVE(ai_get_param,"HI_MPI_AI_GetChnParam");RESOLVE(ai_set_param,"HI_MPI_AI_SetChnParam");RESOLVE(ai_get,"HI_MPI_AI_GetFrame");RESOLVE(ai_release,"HI_MPI_AI_ReleaseFrame");
 return 0;
}
/* Stock HAL hooks handed to ISP_Register_CallBackFunc (libhisdk Isp_drvCreateLib layout:
 * [2]Set3dDenoise [3]Get3dDenoise [6]SetSharpen [7]GetSharpen [8]SemPend [9]SemPost, all (dev,ptr)).
 * NR/sharpen values are kept locally (no VPSS write in this trial); SemPend/SemPost return 0 like stock. */
static unsigned char nr_store[64],sharpen_store[192];static unsigned hook_calls;
static int set_nr(int dev,const void *q){(void)dev;if(!q)return -1;memcpy(nr_store,q,64);hook_calls++;return 0;}
static int get_nr(int dev,void *q){(void)dev;if(!q)return -1;memcpy(q,nr_store,64);hook_calls++;return 0;}
static int set_sharpen(int dev,const void *q){(void)dev;if(!q)return -1;memcpy(sharpen_store,q,192);hook_calls++;return 0;}
static int get_sharpen(int dev,void *q){(void)dev;if(!q)return -1;memcpy(q,sharpen_store,192);hook_calls++;return 0;}
static int sem_noop(int dev){(void)dev;return 0;}
extern void *memalign(usize,usize);
/* Isp_drvInitModule/CreateLib block: {type,mem,size,dev,width,height,0,...}; ISP_Create(devs,&handle,0). */
static unsigned isp_block[16];static void *isp_devs[3];static unsigned isp_handle[8];static int stock_isp;
#define ISTEP(label,call) do{int s_=(call);gm_message(label,s_);if(s_)return -1;}while(0)
static int stock_isp_start(void){
 ISTEP("isp_preinit",a.isp_preinit(0,7,0));
 void *cb[10]={0,0,(void *)set_nr,(void *)get_nr,0,0,(void *)set_sharpen,(void *)get_sharpen,(void *)sem_noop,(void *)sem_noop};
 ISTEP("isp_register_callbacks",a.isp_register_cb(cb));
 memset(isp_block,0,sizeof isp_block);isp_block[0]=7;isp_block[3]=0;isp_block[4]=1920;isp_block[5]=1080;isp_devs[0]=isp_block;
 ISTEP("isp_get_mem_size",a.isp_get_mem(isp_devs));
 if(!isp_block[2]||isp_block[2]>64u*1024*1024){gm_message("isp_mem_size_bounds",(int)isp_block[2]);return -1;}
 void *mem=memalign(32,isp_block[2]);if(!mem){gm_message("isp_mem_alloc",-1);return -1;}memset(mem,0,isp_block[2]);isp_block[1]=(unsigned)(usize)mem;
 char b[96];snprintf(b,sizeof b,"\n{\"kind\":\"media-core\",\"isp_mem_bytes\":%u}\n",isp_block[2]);write_all(1,b,__builtin_strlen(b));
 /* libe3 isp_init_def_capture table: mode 0x12 = 1920x1080 @ 30.0 fps. */
 *a.capture_mode=0x12;
 stock_isp=1;ISTEP("isp_create",a.isp_create(isp_devs,isp_handle,0));
 return 0;
}
static int raw_disable(const char *node,unsigned long bind,const unsigned long *req,unsigned n,const char *label){
 int fd=open(node,2|ARM_SAFE_READ);if(fd<0){gm_message(label,-1);return -1;}unsigned id=0;int r=ioctl(fd,bind,&id);
 for(unsigned i=0;!r&&i<n;i++)r=ioctl(fd,req[i],0);int c=close(fd);gm_message(label,r?r:c);return (r||c)?-1:0;
}
#define STEP(label,call) do{int s_=(call);gm_message(label,s_);if(s_)return -1;}while(0)
/* Brings up video then audio. On failure the caller still calls gm_stop for whatever started. */
int gm_start(const char *dir){
 p=qualified_profile;
 if(load(dir)){gm_message("load",-1);return -1;}
 static const unsigned long ai_req[]={0x5a0bUL,0x5a0dUL,0x5a04UL},ao_req[]={0x5807UL,0x5804UL};
 if(raw_disable("/dev/ao",0x40045800UL,ao_req,2,"takeover_ao")||raw_disable("/dev/ai",0x40045a00UL,ai_req,3,"takeover_ai"))return -1;
 STEP("old_global_sys_exit",a.sys_exit());STEP("old_global_vb_exit",a.vb_exit());
 int fd=open("/dev/vb",2|ARM_SAFE_READ);if(fd<0){gm_message("vb_open",-1);return -1;}int status=ioctl(fd,0x4184420aUL,&p.vb);int closed=close(fd);gm_message("legacy_vb_set",status?status:closed);if(status||closed)return -1;
 STEP("vb_init",a.vb_init());vb=1;STEP("sys_alignment",a.sys_set(&p.alignment));STEP("sys_init",a.sys_init());sys=1;
 fd=open("/dev/hi_mipi",2|ARM_SAFE_READ);if(fd<0){gm_message("receiver_open",-1);return -1;}
 unsigned dev=0;status=ioctl(fd,0x40046d07UL,&dev);if(!status)status=ioctl(fd,0x40046d05UL,&dev);if(!status)status=ioctl(fd,0x40bc6d01UL,&p.receiver);if(!status)usleep(10000);if(!status)status=ioctl(fd,0x40046d08UL,&dev);if(!status)status=ioctl(fd,0x40046d06UL,&dev);closed=close(fd);gm_message("receiver_config",status?status:closed);if(status||closed)return -1;
 STEP("vi_device_config",a.vi_dev_set(0,&p.vi));STEP("vi_device_enable",a.vi_dev_enable(0));vd=1;STEP("vi_channel_config",a.vi_chn_set(0,&p.vi_channel));STEP("vi_channel_enable",a.vi_chn_enable(0));vc=1;
 STEP("sensor_type",a.isp_type(7));if(stock_isp_start())return -1;
 STEP("vpss_create",a.vpss_create(0,&p.vpss));group=1;STEP("vpss_start",a.vpss_start(0));started=1;STEP("vpss_channel_config",a.vpss_set(0,0,&p.vpss_channel));STEP("vpss_channel_mode",a.vpss_mode(0,0,&p.vpss_mode));STEP("vpss_channel_enable",a.vpss_enable(0,0));channel=1;
 STEP("vi_bind",a.bind(&source_vi,&dest_vpss));vi_link=1;STEP("venc_create",a.venc_create(0,&p.encoder));venc=1;STEP("encoder_bind",a.bind(&source_vpss,&dest_venc));enc_link=1;STEP("encoder_receive",a.venc_start(0));receive=1;
 struct aio_attr ai={8000,1,0,0,0,4,320,1,0};
 STEP("ai_attr",a.ai_set(0,&ai));STEP("ai_enable",a.ai_enable(0));ai_dev=1;STEP("ai_chn_enable",a.ai_chn_enable(0,0));ai_chn=1;
 unsigned depth=0;STEP("ai_get_param",a.ai_get_param(0,0,&depth));depth=2;STEP("ai_set_param",a.ai_set_param(0,0,&depth));
 return 0;
}
/* The channel's stream descriptor (readable when an access unit is ready), or -1 if the library has none. */
int gm_video_fd(int c){if(uncertain||!a.venc_getfd)return -1;int fd=a.venc_getfd(c);return fd<0?-1:fd;}
/* One access unit into buf. 0 ok, 1 nothing ready, negative error (stop). *key: an IDR slice is among the
   packs (each pack is one NAL behind a 4-byte start code, so only pack heads are inspected). */
int gm_video_chn(int c,unsigned char *buf,unsigned cap,unsigned *len,u64 *pts,int *key){*key=0;
 if(uncertain)return -9;v3_venc_stat q;memset(&q,0,sizeof q);int st=a.venc_query(c,&q);if(st)return st;if(!q.curPacks)return 1;if(q.curPacks>16)return -2;
 v3_venc_pack packs[16];v3_venc_strm s;memset(packs,0,sizeof packs);memset(&s,0,sizeof s);s.packet=packs;s.count=q.curPacks;st=a.venc_get(c,&s,0);if(st)return st;
 unsigned n=0;int bad=s.packet!=packs||!s.count||s.count>q.curPacks;
 for(unsigned i=0;!bad&&i<s.count;i++){v3_venc_pack *k=&packs[i];if(!k->data||k->offset>k->length||k->length-k->offset>cap-n){bad=1;break;}const unsigned char *d=k->data+k->offset;unsigned m=k->length-k->offset;if(m>4&&!d[0]&&!d[1]&&((!d[2]&&d[3]==1&&(d[4]&31)==5)||(d[2]==1&&(d[3]&31)==5)))*key=1;memcpy(buf+n,d,m);n+=m;}
 *pts=s.count?packs[0].timestamp:0;*len=n;
 st=a.venc_release(c,&s);if(st){uncertain=1;gm_message("venc_release",st);return -9;}
 return bad?-3:0;
}
int gm_video(unsigned char *buf,unsigned cap,unsigned *len,u64 *pts){int key;return gm_video_chn(0,buf,cap,len,pts,&key);}
/* One 40 ms PCM16 mono frame at the current rate into pcm (cap bytes). 0 ok, negative error. */
int gm_audio(unsigned char *pcm,unsigned cap,unsigned *len,u64 *pts){
 if(uncertain)return -9;struct audio_frame f;struct aec_frame e;memset(&f,0,sizeof f);memset(&e,0,sizeof e);
 int st=a.ai_get(0,0,&f,&e,500);if(st)return st;
 int bad=f.width!=1||f.sound!=0||!f.virt[0]||f.length==0||f.length>cap;if(!bad){memcpy(pcm,(const void *)(usize)f.virt[0],f.length);*len=f.length;*pts=f.timestamp;}
 st=a.ai_release(0,0,&f,&e);if(st){uncertain=1;gm_message("ai_release",st);return -9;}
 return bad?-3:0;
}
/* Reverse teardown of whatever started; stops at the first failure (reboot recovers). */
int gm_stop(void){
 if(uncertain){gm_message("lease_uncertain_reset_required",1);return -1;}
#define CLEAN(flag,label,call) do{if(flag){int s_=(call);flag=0;if(s_){gm_message(label,s_);gm_message("teardown_reset_required",1);return -1;}}}while(0)
 CLEAN(ai_chn,"ai_chn_disable",a.ai_chn_disable(0,0));CLEAN(ai_dev,"ai_disable",a.ai_disable(0));
 CLEAN(receive,"venc_stop",a.venc_stop(0));CLEAN(enc_link,"encoder_unbind",a.unbind(&source_vpss,&dest_venc));CLEAN(venc,"venc_destroy",a.venc_destroy(0));CLEAN(channel,"vpss_disable",a.vpss_disable(0,0));CLEAN(vi_link,"vi_unbind",a.unbind(&source_vi,&dest_vpss));CLEAN(started,"vpss_stop",a.vpss_stop(0));CLEAN(group,"vpss_destroy",a.vpss_destroy(0));
 /* Stock ISP threads (ISP_Create) have no qualified stop here: leave VI/SYS/VB for the reboot. */
 /* Stock ISP stop: ISP_SetKeyParam key 1 value 0 -> isp_stop (HI_MPI_ISP_Exit + cancel its two threads),
  * then the remaining VI/SYS/VB teardown. Sensor unregister/isp_exit/join belong to the low-level path only. */
 if(stock_isp){
  char b[96];snprintf(b,sizeof b,"\n{\"kind\":\"media-core\",\"isp_hook_calls\":%u}\n",hook_calls);write_all(1,b,__builtin_strlen(b));
  unsigned off=0;int st=(a.isp_inner&&*a.isp_inner)?a.isp_set_key(*a.isp_inner,1,&off):-1;gm_message("isp_stop",st);
  if(st){gm_message("teardown_reset_required",1);return -1;}
  stock_isp=0;usleep(100000);
 }CLEAN(isp,"isp_exit",a.isp_exit(0));CLEAN(thread,"isp_join",pthread_join(tid,0));CLEAN(sensor,"sensor_unregister",a.sensor_unregister(0));CLEAN(vc,"vi_chn_disable",a.vi_chn_disable(0));CLEAN(vd,"vi_dev_disable",a.vi_dev_disable(0));CLEAN(sys,"sys_exit",a.sys_exit());CLEAN(vb,"vb_exit",a.vb_exit());
 gm_message("teardown_complete",0);return 0;
}

/* Manual AR0237 exposure presets (no 3A): AE registers from ar0237_set_ae_reg's table
 * (0x3012 integration lines, 0x3060 analog gain, 0x305E digital gain, 0x3100 HCG bit2).
 * Writes only after the chip-ID read matches AR0237 (0x3000 == 0x0256); every write is read back. */
static int sensor_ok=-1;
static unsigned rd(unsigned addr){unsigned v=0;if(a.sensor_read(addr,&v))return 0xffffffffu;return v&0xffff;}
int gm_exposure(unsigned preset){
 char b[200];
 if(sensor_ok<0){unsigned id=rd(0x3000);sensor_ok=id==0x0256;snprintf(b,sizeof b,"\n{\"kind\":\"media-core\",\"sensor_id\":%u,\"match\":%s}\n",id,sensor_ok?"true":"false");write_all(1,b,__builtin_strlen(b));}
 if(!sensor_ok)return -1;
 unsigned fll=rd(0x300a),hcg=rd(0x3100);if(fll==0xffffffffu||hcg==0xffffffffu||fll<16||fll>4096)return -2;
 unsigned it=fll-4,again,dgain=0x80,h=hcg&~4u;
 switch(preset){case 0:it=rd(0x3012);again=rd(0x3060);dgain=rd(0x305e);h=hcg;break;
  case 1:again=0x10;break;case 2:again=0x20;h=hcg|4u;break;case 3:again=0x20;h=hcg|4u;dgain=0x100;break;default:return -3;}
 int w=0;if(preset){w|=a.sensor_write(0x3012,it);w|=a.sensor_write(0x3060,again);w|=a.sensor_write(0x305e,dgain);w|=a.sensor_write(0x3100,h);}
 snprintf(b,sizeof b,"\n{\"kind\":\"media-core\",\"exposure_preset\":%u,\"write\":%d,\"fll\":%u,\"r3012\":%u,\"r3060\":%u,\"r305e\":%u,\"r3100\":%u}\n",preset,w,fll,rd(0x3012),rd(0x3060),rd(0x305e),rd(0x3100));write_all(1,b,__builtin_strlen(b));
 return w?-4:0;
}

/* Colour/B&W through the stock ISP context (isplib_inner_param set by ISP_Create); 0 = monochrome. */
int gm_saturation(int level){
 if(!stock_isp||!a.isp_inner||!*a.isp_inner){gm_message("saturation_no_isp",-1);return -1;}
 int r=a.isp_saturation(*a.isp_inner,level);char b[96];snprintf(b,sizeof b,"\n{\"kind\":\"media-core\",\"saturation\":%d,\"status\":%d}\n",level,r);write_all(1,b,__builtin_strlen(b));return r;
}

/* Read-only AE state from the sensor (same registers as ar0237_set_ae_reg's table). */
int gm_ae_state(unsigned *lines,unsigned *fll,unsigned *again,unsigned *dgain,unsigned *hcg){
 if(!stock_isp)return -1;
 if(sensor_ok<0){unsigned id=rd(0x3000);sensor_ok=id==0x0256;}
 if(!sensor_ok)return -2;
 *lines=rd(0x3012);*fll=rd(0x300a);*again=rd(0x3060);*dgain=rd(0x305e);*hcg=rd(0x3100);
 char b[160];snprintf(b,sizeof b,"\n{\"kind\":\"media-core\",\"ae\":[%u,%u,%u,%u,%u]}\n",*lines,*fll,*again,*dgain,*hcg);write_all(1,b,__builtin_strlen(b));
 return (*lines>0xffff||*again>0xffff)?-3:0;
}

/* ---- Hardware bench additions: speaker path exactly as audio-native-trial (AO0 8 kHz, 320 points,
 * DAC unmute via acodec, amplifier via DDM/sound). AO is never disabled: the bench always ends in reboot. */
static int ao_on;static int acodec_fd=-1;
static int acodec(void){if(acodec_fd<0)acodec_fd=open("/dev/acodec",2|ARM_SAFE_READ);return acodec_fd;}
int gm_ao_start(void){
 struct aio_attr ao={8000,1,0,0,0,6,320,1,0};
 STEP("ao_attr",a.ao_set(0,&ao));STEP("ao_enable",a.ao_enable(0));STEP("ao_chn_enable",a.ao_chn_enable(0,0));
 int fd=acodec();if(fd<0){gm_message("dac_unmute",-1);return -1;}
 unsigned z=0;int r1=ioctl(fd,0xc0044112UL,&z);z=0;int r2=ioctl(fd,0xc0044113UL,&z);int c=0;gm_message("dac_unmute",r1?r1:(r2?r2:c));
 if(r1||r2||c)return -1;ao_on=1;return 0;
}
/* One output frame of len bytes (PCM16 mono at the current rate). Blocks up to 1 s. */
int gm_ao_send(const unsigned char *pcm,unsigned len){
 if(!ao_on)return -1;struct audio_frame o;memset(&o,0,sizeof o);o.width=1;o.sound=0;o.virt[0]=(unsigned)(usize)pcm;o.length=len;
 return a.ao_send(0,0,&o,1000);
}
static int dac_unmute(void){
 int fd=acodec();if(fd<0)return -1;
 unsigned z=0;int r1=ioctl(fd,0xc0044112UL,&z);z=0;int r2=ioctl(fd,0xc0044113UL,&z);int c=0;return (r1||r2||c)?-1:0;
}
/* Reconfigure AI0/AO0 at a new rate: channel then device disable, I2S1 selector (24=8k,25=16k,26=32/48k),
 * then the same enable order as start. 40 ms frames (rate/25 points). Caller serializes all audio calls. */
int gm_audio_rate(unsigned rate){
 unsigned sel=rate==8000?24u:rate==16000?25u:(rate==32000||rate==48000)?26u:0u;if(!sel)return -100;
 unsigned points=rate/25;
 if(ai_chn){int s_=a.ai_chn_disable(0,0);gm_message("rate_ai_chn_disable",s_);if(s_)return -101;ai_chn=0;}
 if(ai_dev){int s_=a.ai_disable(0);gm_message("rate_ai_disable",s_);if(s_)return -102;ai_dev=0;}
 if(ao_on){int s1=a.ao_chn_disable(0,0),s2=a.ao_disable(0);gm_message("rate_ao_disable",s1?s1:s2);if(s1||s2)return -103;ao_on=0;}
 int fd=acodec();if(fd<0)return -104;int r=ioctl(fd,0xc0044105UL,&sel);int c=0;gm_message("rate_codec_fs",r?r:c);if(r||c)return -105;
 struct aio_attr ai={rate,1,0,0,0,4,points,1,0},ao={rate,1,0,0,0,6,points,1,0};
 STEP("rate_ai_attr",a.ai_set(0,&ai));STEP("rate_ai_enable",a.ai_enable(0));ai_dev=1;STEP("rate_ai_chn_enable",a.ai_chn_enable(0,0));ai_chn=1;
 unsigned depth=2;STEP("rate_ai_set_param",a.ai_set_param(0,0,&depth));
 STEP("rate_ao_attr",a.ao_set(0,&ao));STEP("rate_ao_enable",a.ao_enable(0));STEP("rate_ao_chn_enable",a.ao_chn_enable(0,0));
 if(dac_unmute())return -106;ao_on=1;return 0;
}
/* acodec volume: which 0 = input (SET_INPUT_VOL), 1 = output (SET_OUTPUT_VOL). */
int gm_codec_set(int which,int db){
 int fd=acodec();if(fd<0)return -1;int v=db;int r=ioctl(fd,which?0xc0044102UL:0xc0044101UL,&v);int c=0;return r?r:c;
}
/* Stock ISP image setters on the live stock ISP context; all take (ctx,int). */
int gm_isp(int kind,int value){
 if(kind<0||kind>8||!stock_isp||!a.isp_inner||!*a.isp_inner)return -100;
 int r=a.isp_fn[kind](*a.isp_inner,value);char b[96];snprintf(b,sizeof b,"\n{\"kind\":\"media-core\",\"isp\":%d,\"value\":%d,\"status\":%d}\n",kind,value,r);write_all(1,b,__builtin_strlen(b));return r;
}
int gm_amp(int on){
 int fd=open("/dev/DDM/sound",2|ARM_SAFE_READ);if(fd<0)return -1;
 unsigned zero[8];memset(zero,0,sizeof zero);int r=ioctl(fd,on?0x401c5308UL:0x401c5307UL,zero);int c=close(fd);return (r||c)?-1:0;
}
int gm_codec_volume(int *in,int *out){
 int fd=acodec();if(fd<0)return -1;
 *in=-999;*out=-999;int r1=ioctl(fd,0xc0044103UL,in),r2=ioctl(fd,0xc0044104UL,out);int c=0;return (r1||r2||c)?-1:0;
}

/* ---- Stream settings: recreate VENC chn (0 main, 1 sub) from the qualified profile encoder with new
 * size/fps/bitrate/gop/rate mode; VPSS chn of the same index is (re)sized when needed.
 * Caller must hold its video lock for chn (no concurrent gm_video_chn on that channel). */
static v3_venc_chn cur_enc[2];static int enc_live[2]={0,0},vchn_live[2]={0,0};static unsigned cur_w[2],cur_h[2];
static int src_chn[2]={0,1};static unsigned vpss_w[3],vpss_h[3];
static int venc_up(int c,v3_venc_chn *e){
 v3_sys_bind vs={V3_SYS_MOD_VPSS,0,src_chn[c]},ve={V3_SYS_MOD_VENC,0,c};
 int r=a.venc_create(c,e);gm_message("stream_venc_create",r);if(r)return -110;
 r=a.bind(&vs,&ve);gm_message("stream_bind",r);if(r){a.venc_destroy(c);return -111;}
 r=a.venc_start(c);gm_message("stream_venc_start",r);if(r){a.unbind(&vs,&ve);a.venc_destroy(c);return -112;}
 cur_enc[c]=*e;enc_live[c]=1;if(c==0){receive=enc_link=venc=1;}return 0;
}
int gm_stream_set(int c,unsigned w,unsigned h,unsigned fps,unsigned kbps,unsigned gop,int vbr,unsigned profile){
 if(c<0||c>1||w<160||h<120||w>1920||h>1080||(w&15)||(h&7)||fps<1||fps>30||kbps<32||kbps>16384||gop<1||gop>300||profile>2)return -100;
 if(c==0&&!enc_live[0]){enc_live[0]=receive&&enc_link&&venc;vchn_live[0]=channel;cur_enc[0]=p.encoder;cur_w[0]=1920;cur_h[0]=1080;}
 v3_sys_bind vs={V3_SYS_MOD_VPSS,0,src_chn[c]},ve={V3_SYS_MOD_VENC,0,c};
 int had=enc_live[c];v3_venc_chn prev=cur_enc[c];unsigned pw=cur_w[c],ph=cur_h[c];int psrc=src_chn[c];
 if(enc_live[c]){int r1=a.venc_stop(c),r2=a.unbind(&vs,&ve),r3=a.venc_destroy(c);gm_message("stream_teardown",r1?r1:r2?r2:r3);if(r1||r2||r3)return -101;enc_live[c]=0;if(c==0){receive=enc_link=venc=0;}}
 /* VPSS chn 0 stays 1920x1080 (it does not feed VENC when resized); scaled main uses physical chn 2. */
 int src=c==1?1:(w==1920&&h==1080)?0:2;
 if(src!=0&&(vpss_w[src]!=w||vpss_h[src]!=h)){
  if(vpss_w[src]){int r=a.vpss_disable(0,src);gm_message("stream_vpss_disable",r);if(r)return -102;vpss_w[src]=0;}
  v3_vpss_mode m=p.vpss_mode;m.userModeOn=1;m.dest.width=w;m.dest.height=h;
  STEP("stream_vpss_chn",a.vpss_set(0,src,&p.vpss_channel));STEP("stream_vpss_mode",a.vpss_mode(0,src,&m));STEP("stream_vpss_enable",a.vpss_enable(0,src));
  vpss_w[src]=w;vpss_h[src]=h;
 }
 src_chn[c]=src;cur_w[c]=w;cur_h[c]=h;
 v3_venc_chn e=p.encoder;e.attrib.codec=V3_VENC_CODEC_H264;
 e.attrib.h264.maxPic.width=e.attrib.h264.pic.width=w;e.attrib.h264.maxPic.height=e.attrib.h264.pic.height=h;
 e.attrib.h264.bufSize=w*h;e.attrib.h264.profile=profile;
 unsigned stat=10;
 if(vbr){e.rate.mode=V3_VENC_RATEMODE_H264VBR;memset(&e.rate.h264Vbr,0,sizeof e.rate.h264Vbr);e.rate.h264Vbr.gop=gop;e.rate.h264Vbr.statTime=stat;e.rate.h264Vbr.srcFps=30;e.rate.h264Vbr.dstFps=fps;e.rate.h264Vbr.maxBitrate=kbps;e.rate.h264Vbr.maxQual=50;e.rate.h264Vbr.minQual=10;e.rate.h264Vbr.minIQual=10;}
 else{e.rate.mode=V3_VENC_RATEMODE_H264CBR;memset(&e.rate.h264Cbr,0,sizeof e.rate.h264Cbr);e.rate.h264Cbr.gop=gop;e.rate.h264Cbr.statTime=stat;e.rate.h264Cbr.srcFps=30;e.rate.h264Cbr.dstFps=fps;e.rate.h264Cbr.bitrate=kbps;e.rate.h264Cbr.avgLvl=1;}
 int rc=venc_up(c,&e);if(!rc)return 0;
 /* Fallback: bring back the previous working encoder (and its VPSS size) so recording never stays off. */
 if(had){
  src_chn[c]=psrc;cur_w[c]=pw;cur_h[c]=ph;
  int r2=venc_up(c,&prev);gm_message("stream_fallback",r2);if(!r2)return rc-1000;
 }
 return rc;
}

/* Report the live encoder: w,h,fps,kbps,gop,mode(1 CBR,2 VBR),profile; returns -1 when the channel is off. */
int gm_stream_get(int c,unsigned *out){
 if(c<0||c>1)return -1;if(c==0&&!enc_live[0]){cur_enc[0]=p.encoder;cur_w[0]=p.encoder.attrib.h264.pic.width;cur_h[0]=p.encoder.attrib.h264.pic.height;if(receive)enc_live[0]=1;}
 if(!enc_live[c])return -1;v3_venc_chn *e=&cur_enc[c];
 out[0]=e->attrib.h264.pic.width;out[1]=e->attrib.h264.pic.height;out[6]=e->attrib.h264.profile;out[5]=e->rate.mode;
 if(e->rate.mode==V3_VENC_RATEMODE_H264VBR){out[2]=e->rate.h264Vbr.dstFps;out[3]=e->rate.h264Vbr.maxBitrate;out[4]=e->rate.h264Vbr.gop;}
 else{out[2]=e->rate.h264Cbr.dstFps;out[3]=e->rate.h264Cbr.bitrate;out[4]=e->rate.h264Cbr.gop;}
 return 0;
}
