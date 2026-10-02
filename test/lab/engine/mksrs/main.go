// Command mksrs (prototype lab) writes a sing-box binary rule set matching
// the given domain suffixes and CIDRs: mksrs out.srs suffix|cidr...
package main

import (
	"net/netip"
	"os"

	"github.com/sagernet/sing-box/common/srs"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
)

func main() {
	var rule option.DefaultHeadlessRule
	for _, arg := range os.Args[2:] {
		if _, err := netip.ParsePrefix(arg); err == nil {
			rule.IPCIDR = append(rule.IPCIDR, arg)
		} else {
			rule.DomainSuffix = append(rule.DomainSuffix, arg)
		}
	}
	file, err := os.Create(os.Args[1])
	if err != nil {
		panic(err)
	}
	defer file.Close()
	set := option.PlainRuleSet{Rules: []option.HeadlessRule{{Type: C.RuleTypeDefault, DefaultOptions: rule}}}
	if err = srs.Write(file, set, C.RuleSetVersionCurrent); err != nil {
		panic(err)
	}
}
