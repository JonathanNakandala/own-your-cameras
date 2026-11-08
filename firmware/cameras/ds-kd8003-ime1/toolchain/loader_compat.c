/* libloader_compat.so: NEEDED before libc by the daemon so these definitions win global symbol lookup.
 * 1. ioctl: the standalone libmpi's VB pool-info request 0xc014420b -> the stock driver's 0xc010420b.
 *    The loaded libmpi also numbers four requests differently from the stock driver, which it
 *    passes the caller's pointer for unchanged: VENC Set/GetChnAttr 0x67/0x68 -> 0x63/0x64 (88 B)
 *    and VPSS Set/GetGrpSharpen (240 B) -> the stock 192-B requests; callers pass stock layouts.
 *    Its own 0x63/0x64 (super-frame config, 20 B) are never called here. A request is rewritten
 *    only on its own node (a device file named vb, venc or vpss under /dev, wherever a link points);
 *    another driver's equal number passes unchanged. An fd that cannot be read back (no /proc) is
 *    rewritten as before.
 * 2. pthreadSpawn/setPthreadName/pthreadCancel: the hicore helpers stock libe3_isp_access imports,
 *    reproduced from hicore 0x254668: (out, priority, stack, entry, nargs<=10, args...);
 *    nargs<=1 -> pthread_create(entry, arg0); otherwise entry(arg0..arg9) via a trampoline.
 *    Priority is not applied (default scheduling); stack is honoured with a 64 KiB floor. */
typedef unsigned u32;typedef unsigned long usize;
extern int *__errno_location(void);
extern int pthread_create(unsigned long *,const void *,void *(*)(void *),void *),pthread_attr_init(void *),pthread_attr_destroy(void *),pthread_attr_setstacksize(void *,usize),pthread_cancel(unsigned long),prctl(int,...);
extern void *malloc(usize);extern void free(void *);extern long readlink(const char *,char *,usize);
static long sys_ioctl(long fd,long req,long arg){
 register long r0 __asm__("r0")=fd;register long r1 __asm__("r1")=req;register long r2 __asm__("r2")=arg;register long r7 __asm__("r7")=54;
 __asm__ volatile("svc #0":"+r"(r0):"r"(r1),"r"(r2),"r"(r7):"memory");return r0;
}
static int finish(long r){if(r<0&&r>-4096){*__errno_location()=(int)-r;return -1;}return (int)r;}
/* Counters bumped from any thread: ARMv5 has no exclusive loads/stores, so the kernel's cmpxchg
 * helper (CONFIG_KUSER_HELPERS) makes the increment atomic. */
typedef int(*kuser_cmpxchg_t)(unsigned,unsigned,volatile unsigned *);
#define KUSER_CMPXCHG ((kuser_cmpxchg_t)0xffff0fc0)
static volatile unsigned translated,spawned;
static void bump(volatile unsigned *n){for(;;){unsigned v=*n;if(!KUSER_CMPXCHG(v,v+1,n))return;}}
unsigned audio_compat_translations(void){return translated;}
unsigned loader_compat_spawned(void){return spawned;}
/* 1 when fd is a /dev file whose last path part is <name>, 0 when it is another file, -1 when
 * that cannot be read. */
static int node_is(int fd,const char *name){
 if(fd<0)return 0;
 char path[32]="/proc/self/fd/",digits[12];int n=14,k=0;unsigned v=(unsigned)fd;
 do{digits[k++]=(char)('0'+v%10);v/=10;}while(v);while(k)path[n++]=digits[--k];path[n]=0;
 char t[32];long len=readlink(path,t,sizeof t-1);if(len<=0)return -1;t[len]=0;
 if(t[0]!='/'||t[1]!='d'||t[2]!='e'||t[3]!='v'||t[4]!='/')return 0;
 const char *a=t+5;for(const char *c=a;*c;c++)if(*c=='/')a=c+1;
 while(*name&&*a==*name){a++;name++;}return !*a&&!*name;
}
int ioctl(int fd,unsigned long req,...){
 __builtin_va_list ap;__builtin_va_start(ap,req);void *arg=__builtin_va_arg(ap,void *);__builtin_va_end(ap);
 if(req==0xc014420bUL&&node_is(fd,"vb")){if(!arg){*__errno_location()=14;return -1;}u32 *p=arg;u32 s[4]={p[0],0,0,0};int r=finish(sys_ioctl(fd,(long)0xc010420bUL,(long)s));if(r==0){p[1]=s[1];p[2]=s[2];p[3]=0;bump(&translated);}return r;}
 unsigned long stock=req==0x40584567UL?0x40584563UL:req==0xc0584568UL?0xc0584564UL:req==0x40f05005UL?0x40c05005UL:req==0x80f05006UL?0x80c05006UL:0;
 if(stock&&node_is(fd,(stock&0xff00UL)==0x4500UL?"venc":"vpss")){bump(&translated);req=stock;}
 return finish(sys_ioctl(fd,(long)req,(long)arg));
}
struct spawn {void *(*entry)(usize,usize,usize,usize,usize,usize,usize,usize,usize,usize);usize arg[10];};
static void *trampoline(void *raw){struct spawn s=*(struct spawn *)raw;free(raw);return s.entry(s.arg[0],s.arg[1],s.arg[2],s.arg[3],s.arg[4],s.arg[5],s.arg[6],s.arg[7],s.arg[8],s.arg[9]);}
int pthreadSpawn(unsigned long *out,int priority,unsigned stack,void *entry,unsigned nargs,...){
 (void)priority;if(!entry||nargs>10)return 22;
 unsigned long local;if(!out)out=&local;
 __builtin_va_list ap;__builtin_va_start(ap,nargs);usize args[10]={0};for(unsigned i=0;i<nargs;i++)args[i]=__builtin_va_arg(ap,usize);__builtin_va_end(ap);
 unsigned char attr[64];if(pthread_attr_init(attr))return 22;pthread_attr_setstacksize(attr,stack<65536?65536:stack);
 int r;
 if(nargs<=1)r=pthread_create(out,attr,(void *(*)(void *))entry,(void *)args[0]);
 else{struct spawn *s=malloc(sizeof *s);if(!s){pthread_attr_destroy(attr);return 12;}s->entry=entry;for(unsigned i=0;i<10;i++)s->arg[i]=args[i];r=pthread_create(out,attr,trampoline,s);if(r)free(s);}
 pthread_attr_destroy(attr);if(!r)bump(&spawned);return r;
}
int setPthreadName(const char *name){return name?prctl(15,name,0,0,0):0;}
int pthreadCancel(unsigned long thread){return pthread_cancel(thread);}
