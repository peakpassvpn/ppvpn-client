// Runs a Rust body that sends Objective-C messages (autostart.rs) and hands
// back the exception it raised instead of letting it unwind on into Rust:
// there, the bridge's catch_unwind aborts on a foreign exception.
#import <Foundation/Foundation.h>
#include <string.h>

// NULL when `body` returned; otherwise "<name>: <reason>", from strdup (the
// caller frees it).
char *ppvpn_objc_catch(void (*body)(void *), void *context) {
  @try {
    body(context);
    return NULL;
  } @catch (NSException *exception) {
    const char *text = [NSString stringWithFormat:@"%@: %@", exception.name, exception.reason].UTF8String;
    return strdup(text ? text : "Objective-C exception");
  } @catch (id other) {
    return strdup("Objective-C exception");
  }
}
