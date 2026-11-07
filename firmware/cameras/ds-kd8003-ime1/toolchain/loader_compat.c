/* libloader_compat.so: NEEDED before libc by the daemon so these definitions win global symbol lookup.
 * 1. ioctl: the standalone libmpi's VB pool-info request 0xc014420b -> the stock driver's 0xc010420b.
 * 2. pthreadSpawn/setPthreadName/pthreadCancel: the hicore helpers stock libe3_isp_access imports,
 *    reproduced from hicore 0x254668: (out, priority, stack, entry, nargs<=10, args...);
 *    nargs<=1 -> pthread_create(entry, arg0); otherwise entry(arg0..arg9) via a trampoline.
 *    Priority is not applied (default scheduling); stack is honoured with a 64 KiB floor. */
typedef unsigned u32;typedef unsigned long usize;
extern int *__errno_location(void);
extern int pthread_create(unsigned long *,const void *,void *(*)(void *),void *),pthread_attr_init(void *),pthread_attr_destroy(void *),pthread_attr_setstacksize(void *,usize),pthread_cancel(unsigned long),prctl(int,...);
extern void *malloc(usize);extern void free(void *);
static long sys_ioctl(long fd,long req,long arg){
 register long r0 __asm__("r0")=fd;register long r1 __asm__("r1")=req;register long r2 __asm__("r2")=arg;register long r7 __asm__("r7")=54;
 __asm__ volatile("svc #0":"+r"(r0):"r"(r1),"r"(r2),"r"(r7):"memory");return r0;
}
static int finish(long r){if(r<0&&r>-4096){*__errno_location()=(int)-r;return -1;}return (int)r;}
static unsigned translated,spawned;
unsigned audio_compat_translations(void){return translated;}
unsigned loader_compat_spawned(void){return spawned;}
int ioctl(int fd,unsigned long req,...){
 __builtin_va_list ap;__builtin_va_start(ap,req);void *arg=__builtin_va_arg(ap,void *);__builtin_va_end(ap);
 if(req==0xc014420bUL){if(!arg){*__errno_location()=14;return -1;}u32 *p=arg;u32 s[4]={p[0],0,0,0};int r=finish(sys_ioctl(fd,(long)0xc010420bUL,(long)s));if(r==0){p[1]=s[1];p[2]=s[2];p[3]=0;translated++;}return r;}
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
 pthread_attr_destroy(attr);if(!r)spawned++;return r;
}
int setPthreadName(const char *name){return name?prctl(15,name,0,0,0):0;}
int pthreadCancel(unsigned long thread){return pthread_cancel(thread);}
