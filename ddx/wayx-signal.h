/* SIGTERM can arrive on the UI thread while the X worker sleeps indefinitely.
 * Wake the server's wait set with an async-signal-safe self-pipe, not an X11 query. */
#ifndef WIN32
#include <signal.h>
#include <unistd.h>
#include <fcntl.h>
static int signal_read_fd=-1;
static volatile sig_atomic_t signal_write_fd=-1;
static struct sigaction previous_term,previous_int;
static Bool signal_handlers_installed;
static void wayx_terminate_signal(int sig) {
    int saved=errno;
    GiveUp(sig);
    unsigned char byte=1;
    int fd=signal_write_fd;
    if(fd>=0) {ssize_t result;do {result=write(fd,&byte,1);} while(result<0 && errno==EINTR);}
    errno=saved;
}
static void signal_ready(int fd,int ready,void *data) {
    unsigned char bytes[32];
    while(read(fd,bytes,sizeof(bytes))>0) {}
}
static void install_signal_wake(void) {
    int fds[2];
    if(pipe(fds)<0) FatalError("WayX could not create signal wake pipe\n");
    for(unsigned i=0;i<2;i++)
        if(fcntl(fds[i],F_SETFL,O_NONBLOCK)<0 || fcntl(fds[i],F_SETFD,FD_CLOEXEC)<0)
            FatalError("WayX could not configure signal wake pipe\n");
    signal_read_fd=fds[0];signal_write_fd=fds[1];
    if(!SetNotifyFd(signal_read_fd,signal_ready,X_NOTIFY_READ,NULL))
        FatalError("WayX could not register signal wake pipe\n");
    struct sigaction action={0};
    sigemptyset(&action.sa_mask);
    sigaddset(&action.sa_mask,SIGTERM);sigaddset(&action.sa_mask,SIGINT);
    action.sa_handler=wayx_terminate_signal;
    if(sigaction(SIGTERM,&action,&previous_term)<0 || sigaction(SIGINT,&action,&previous_int)<0)
        FatalError("WayX could not register termination signal wake\n");
    signal_handlers_installed=TRUE;
}
static void remove_signal_wake(void) {
    if(signal_handlers_installed) {
        sigaction(SIGTERM,&previous_term,NULL);sigaction(SIGINT,&previous_int,NULL);
        signal_handlers_installed=FALSE;
    }
    int writer=signal_write_fd;signal_write_fd=-1;
    if(signal_read_fd>=0) {RemoveNotifyFd(signal_read_fd);close(signal_read_fd);signal_read_fd=-1;}
    if(writer>=0) close(writer);
}
#else
static void install_signal_wake(void) {}
static void remove_signal_wake(void) {}
#endif
