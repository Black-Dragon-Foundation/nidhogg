// S6 probe: decode each file with CGImageSource into premultiplied RGBA8 sRGB, and with NSImage's bitmap rep into the
// same format; print size and the largest per-channel difference.  The two share ImageIO underneath; the probe shows
// what a plain CGImageSource route supports (PNG, JPEG, GIF, TIFF, WebP, HEIC) and how close it is to NS's decode.
#import <AppKit/AppKit.h>
#import <ImageIO/ImageIO.h>

static NSData *render(CGImageRef img, size_t w, size_t h) {
  CGColorSpaceRef cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
  NSMutableData *d = [NSMutableData dataWithLength:w * h * 4];
  CGContextRef c = CGBitmapContextCreate(d.mutableBytes, w, h, 8, w * 4, cs,
                                         kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
  CGContextDrawImage(c, CGRectMake(0, 0, w, h), img);
  CGContextRelease(c); CGColorSpaceRelease(cs);
  return d;
}

int main(int argc, char **argv) {
  @autoreleasepool {
    for (int i = 1; i < argc; i++) {
      NSString *p = [NSString stringWithUTF8String:argv[i]];
      NSString *ext = p.pathExtension.lowercaseString;
      if ([@[@"pbm", @"xpm", @"svg", @"xbm"] containsObject:ext]) continue;
      NSData *data = [NSData dataWithContentsOfFile:p];
      if (!data) continue;
      CGImageSourceRef src = CGImageSourceCreateWithData((__bridge CFDataRef)data, NULL);
      CGImageRef a = src ? CGImageSourceCreateImageAtIndex(src, 0, NULL) : NULL;
      if (!a) { printf("%s: CGImageSource cannot decode (%s)\n", argv[i], src ? "no image" : "no source"); continue; }
      size_t w = CGImageGetWidth(a), h = CGImageGetHeight(a);
      NSData *da = render(a, w, h);
      NSImage *ni = [[NSImage alloc] initWithData:data];
      NSBitmapImageRep *rep = [NSBitmapImageRep imageRepWithData:data];
      CGImageRef b = rep ? [rep CGImage] : [ni CGImageForProposedRect:NULL context:nil hints:nil];
      if (!b) { printf("%s: %zux%zu, NSImage cannot decode\n", argv[i], w, h); continue; }
      NSData *db = render(b, w, h);
      int maxd = 0; const unsigned char *x = da.bytes, *y = db.bytes;
      for (size_t k = 0; k < da.length; k++) { int dd = abs((int)x[k] - (int)y[k]); if (dd > maxd) maxd = dd; }
      printf("%s: %zux%zu, type %s, max channel diff %d\n", argv[i], w, h, CGImageSourceGetType(src) ? [(__bridge NSString *)CGImageSourceGetType(src) UTF8String] : "?", maxd);
    }
  }
  return 0;
}
