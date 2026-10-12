import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))
from bracketbot_tracking_led import tracking_lost

class TrackingLed(unittest.TestCase):
    def test_loss_and_recovery(self):
        now=1_000_000_000
        healthy=dict(vo_lost=False,stalled=False,localized=True,degraded=True)
        self.assertFalse(tracking_lost(healthy,now,now,now))
        for k,v in [('vo_lost',True),('stalled',True),('localized',False)]:
            self.assertTrue(tracking_lost(dict(healthy,**{k:v}),now,now,now))
        self.assertTrue(tracking_lost(healthy,now-800_000_001,now,now))
        self.assertTrue(tracking_lost(healthy,now,now-800_000_001,now))
        self.assertTrue(tracking_lost(healthy,now+100_000_001,now,now))
        self.assertTrue(tracking_lost(None,None,None,now))

if __name__=='__main__':unittest.main()
