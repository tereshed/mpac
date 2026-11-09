#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <TargetConditionals.h>
#include <unistd.h>
#if TARGET_OS_IOS
#import <UIKit/UIKit.h>
#endif

static AVAudioRecorder *recorder;
static NSURL *recordingURL;

static void discard(void) {
    if (recorder == nil && recordingURL == nil) return;
    [recorder stop];
    recorder = nil;
    if (recordingURL != nil) {
        [[NSFileManager defaultManager] removeItemAtURL:recordingURL error:nil];
        recordingURL = nil;
    }
#if TARGET_OS_IOS
    [[AVAudioSession sharedInstance] setActive:NO error:nil];
#endif
}

// Rust calls these functions on worker threads. AVAudioRecorder and its
// lifecycle notifications are confined to the main queue.
bool mpac_voice_authorize(void) {
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    __block BOOL allowed = NO;
    dispatch_async(dispatch_get_main_queue(), ^{
#if TARGET_OS_IOS
        [[AVAudioSession sharedInstance] requestRecordPermission:^(BOOL granted) {
#else
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {
#endif
            allowed = granted;
            dispatch_semaphore_signal(done);
        }];
    });
    return dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC)) == 0 && allowed;
}

bool mpac_voice_start(void) {
    __block BOOL started = NO;
    dispatch_sync(dispatch_get_main_queue(), ^{
        discard();
#if TARGET_OS_IOS
        if ([UIApplication sharedApplication].applicationState == UIApplicationStateBackground) return;
        AVAudioSession *session = [AVAudioSession sharedInstance];
        if (![session setCategory:AVAudioSessionCategoryRecord mode:AVAudioSessionModeDefault options:0 error:nil]
            || ![session setActive:YES error:nil]) return;
#endif
        static dispatch_once_t observers;
        dispatch_once(&observers, ^{
#if TARGET_OS_IOS
            NSArray *names = @[@"UIApplicationDidEnterBackgroundNotification", AVAudioSessionInterruptionNotification];
#else
            NSArray *names = @[@"NSApplicationDidHideNotification", @"NSApplicationWillTerminateNotification"];
#endif
            for (NSString *name in names) {
                [[NSNotificationCenter defaultCenter] addObserverForName:name object:nil queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *notification) { (void)notification; discard(); }];
            }
        });
        // mkstemps creates an owner-only file before AVAudioRecorder opens it.
        NSString *pattern = [NSTemporaryDirectory() stringByAppendingPathComponent:@"mpac-voice-XXXXXX.m4a"];
        char *path = strdup(pattern.fileSystemRepresentation);
        if (path == NULL) return;
        int fd = mkstemps(path, 4);
        if (fd >= 0) {
            close(fd);
            recordingURL = [NSURL fileURLWithFileSystemRepresentation:path isDirectory:NO relativeToURL:nil];
        }
        free(path);
        if (recordingURL == nil) {
#if TARGET_OS_IOS
            [session setActive:NO error:nil];
#endif
            return;
        }
        recorder = [[AVAudioRecorder alloc] initWithURL:recordingURL settings:@{
            AVFormatIDKey: @(kAudioFormatMPEG4AAC),
            AVSampleRateKey: @24000,
            AVNumberOfChannelsKey: @1,
            AVEncoderBitRateKey: @48000
        } error:nil];
        started = [recorder recordForDuration:60.0];
        if (!started) discard();
        else {
            NSURL *current = recordingURL;
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 120 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                if (recordingURL == current) discard();
            });
        }
    });
    return started;
}

void *mpac_voice_stop(size_t *length) {
    __block void *bytes = NULL;
    dispatch_sync(dispatch_get_main_queue(), ^{
        [recorder stop];
        NSNumber *size = nil;
        [recordingURL getResourceValue:&size forKey:NSURLFileSizeKey error:nil];
        if (size.unsignedLongLongValue > 0 && size.unsignedLongLongValue <= 8 * 1024 * 1024) {
            NSData *data = [NSData dataWithContentsOfURL:recordingURL];
            if (data.length > 0 && data.length <= 8 * 1024 * 1024) {
                bytes = malloc(data.length);
                if (bytes != NULL) {
                    memcpy(bytes, data.bytes, data.length);
                    *length = data.length;
                }
            }
        }
        discard();
    });
    return bytes;
}

void mpac_voice_cancel(void) {
    dispatch_sync(dispatch_get_main_queue(), ^{ discard(); });
}
