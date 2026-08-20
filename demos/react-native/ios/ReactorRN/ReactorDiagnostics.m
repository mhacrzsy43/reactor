#import <Foundation/Foundation.h>
#import <React/RCTBridgeModule.h>
#import <mach/mach_time.h>

@interface ReactorDiagnostics : NSObject <RCTBridgeModule>
@end

@implementation ReactorDiagnostics {
  NSLock *_writeLock;
}

RCT_EXPORT_MODULE(ReactorDiagnostics)

+ (BOOL)requiresMainQueueSetup
{
  return NO;
}

- (instancetype)init
{
  if ((self = [super init])) {
    _writeLock = [NSLock new];
  }
  return self;
}

- (NSURL *)reactorDirectory
{
  NSFileManager *files = NSFileManager.defaultManager;
  NSURL *applicationSupport = [files URLsForDirectory:NSApplicationSupportDirectory
                                             inDomains:NSUserDomainMask].firstObject;
  NSURL *directory = [applicationSupport URLByAppendingPathComponent:@"reactor" isDirectory:YES];
  [files createDirectoryAtURL:directory
  withIntermediateDirectories:YES
                   attributes:nil
                        error:nil];
  return directory;
}

- (NSURL *)artifactURL:(NSString *)name
{
  return [[self reactorDirectory] URLByAppendingPathComponent:name isDirectory:NO];
}

- (NSDictionary *)sandboxPaths
{
  return @{
    @"root": [self reactorDirectory].path,
    @"events": [self artifactURL:@"rn-diagnostics.ndjson"].path,
    @"reactDevToolsProfile": [self artifactURL:@"rn-react-devtools-profile.json"].path,
    @"hermesHeapStats": [self artifactURL:@"rn-hermes-heap-stats.ndjson"].path,
    @"hermesHeapSnapshot": [self artifactURL:@"rn-hermes.heapsnapshot"].path,
    @"hermesCpuProfile": [self artifactURL:@"rn-hermes-cpu.trace.json"].path,
  };
}

- (NSDictionary *)constantsToExport
{
  return @{
    @"diagnosticBuild": @NO,
    @"sdkVersion": @"1.0.0",
    @"protocolVersion": @1,
    @"capabilities": @[
      @"react-profiler",
      @"runtime-events",
      @"react-devtools-profile",
      @"hermes-heap-unavailable",
      @"hermes-cpu-unavailable",
    ],
    @"sandboxPaths": [self sandboxPaths],
    @"availability": @{
      @"hermesHeap": @{
        @"status": @"unavailable",
        @"reason": @"RN 0.87 does not expose a supported iOS runtime executor to this legacy native module",
      },
      @"hermesCpuSampling": @{
        @"status": @"unavailable",
        @"reason": @"No public iOS Hermes sampling-profiler bridge API is available to this module",
      },
      @"reactDevToolsProfile": @{
        @"status": @"available",
        @"classification": @"devtools-6",
      },
    },
  };
}

- (uint64_t)continuousTimeNanos
{
  static mach_timebase_info_data_t timebase;
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    mach_timebase_info(&timebase);
  });
  return mach_continuous_time() * timebase.numer / timebase.denom;
}

- (void)withWriteLock:(void (^)(void))operation
{
  [_writeLock lock];
  @try {
    operation();
  } @finally {
    [_writeLock unlock];
  }
}

RCT_EXPORT_METHOD(reset)
{
  NSArray<NSString *> *names = @[
    @"rn-diagnostics.ndjson",
    @"rn-react-devtools-profile.json",
    @"rn-hermes-heap-stats.ndjson",
    @"rn-hermes.heapsnapshot",
    @"rn-hermes-cpu.trace.json",
  ];
  [self withWriteLock:^{
    NSFileManager *files = NSFileManager.defaultManager;
    for (NSString *name in names) {
      [files removeItemAtURL:[self artifactURL:name] error:nil];
    }
    [NSData.data writeToURL:[self artifactURL:@"rn-diagnostics.ndjson"] atomically:YES];
  }];
}

RCT_EXPORT_METHOD(appendEvent:(NSString *)kind payloadJson:(NSString *)payloadJson)
{
  NSRegularExpression *allowed =
      [NSRegularExpression regularExpressionWithPattern:@"^[a-z_]{1,48}$" options:0 error:nil];
  if (payloadJson.length > 64 * 1024 ||
      [allowed numberOfMatchesInString:kind options:0 range:NSMakeRange(0, kind.length)] != 1) {
    return;
  }
  NSData *payloadData = [payloadJson dataUsingEncoding:NSUTF8StringEncoding];
  if (payloadData == nil || [NSJSONSerialization JSONObjectWithData:payloadData options:0 error:nil] == nil) {
    return;
  }
  NSString *line = [NSString stringWithFormat:
      @"{\"schemaVersion\":1,\"kind\":\"%@\",\"timestampMs\":%.0f,\"elapsedRealtimeNanos\":%llu,\"payload\":%@}\n",
      kind,
      NSDate.date.timeIntervalSince1970 * 1000.0,
      [self continuousTimeNanos],
      payloadJson];
  NSData *data = [line dataUsingEncoding:NSUTF8StringEncoding];
  [self withWriteLock:^{
    NSURL *target = [self artifactURL:@"rn-diagnostics.ndjson"];
    if (![NSFileManager.defaultManager fileExistsAtPath:target.path]) {
      [data writeToURL:target atomically:YES];
      return;
    }
    NSFileHandle *handle = [NSFileHandle fileHandleForWritingToURL:target error:nil];
    [handle seekToEndOfFile];
    [handle writeData:data];
    [handle closeFile];
  }];
}

RCT_EXPORT_METHOD(writeProfile:(NSString *)profileJson)
{
  if (profileJson.length > 16 * 1024 * 1024) {
    return;
  }
  NSData *data = [profileJson dataUsingEncoding:NSUTF8StringEncoding];
  if (data == nil || [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] == nil) {
    return;
  }
  [self withWriteLock:^{
    [data writeToURL:[self artifactURL:@"rn-react-devtools-profile.json"] atomically:YES];
  }];
}

RCT_EXPORT_METHOD(captureHermesHeap:(NSString *)label snapshot:(BOOL)snapshot)
{
  if (label.length > 96) {
    return;
  }
  NSDictionary *payload = @{
    @"label": label,
    @"snapshot": @(snapshot),
    @"status": @"unavailable",
    @"reason": @"supported iOS JSI instrumentation access is unavailable in this RN 0.87 bridge",
  };
  NSData *data = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
  NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
  [self appendEvent:@"hermes_heap" payloadJson:json];
}

@end
