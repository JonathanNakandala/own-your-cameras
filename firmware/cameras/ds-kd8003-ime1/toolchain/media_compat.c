/* libnative_compat.so: the MPP V1 symbols the stock libe3_isp_access imports from hicore and the
 * standalone libmpi lacks. Legacy register wrappers over HI_MPI_SYS_Mmap and the thread-name/cancel
 * helpers. No constructors and no fabricated success; opaque vendor thread factories are refused.
 */
typedef unsigned u32;
extern void *HI_MPI_SYS_Mmap(u32,u32);
extern int HI_MPI_SYS_Munmap(void *,u32),prctl(int,...),pthread_cancel(unsigned long);
int HI_MPI_SYS_GetReg(u32 physical,u32 *value) {
 if(!value)return (int)0xa0028006u;
 if(physical&3)return -1;
 volatile u32 *p=HI_MPI_SYS_Mmap(physical,4);
 if(!p||p==(void *)-1)return -1;
 *value=*p;return HI_MPI_SYS_Munmap((void *)p,4);
}
int HI_MPI_SYS_SetReg(u32 physical,u32 value) {
 if(physical&3)return -1;
 volatile u32 *p=HI_MPI_SYS_Mmap(physical,4);
 if(!p||p==(void *)-1)return -1;
 *p=value;return HI_MPI_SYS_Munmap((void *)p,4);
}
int pthreadCancel(unsigned long thread){return pthread_cancel(thread);}
int setPthreadName(const char *name){return name?prctl(15,name,0,0,0):0;}
/* Intentionally unsupported. The native application owns an explicit joined
 * pthread for HI_MPI_ISP_Run; it never invokes ISP_Create/PreInit/inner loop.
 * Returning ENOTSUP, rather than pretending a worker exists, is fail-closed. */
int pthreadSpawn(unsigned long *out,int priority,unsigned stack,void *entry,unsigned nargs,...) {
 (void)out;(void)priority;(void)stack;(void)entry;(void)nargs;return 95;
}
