import util.geo
from helpers import do_thing
from . import config
import os


def main():
    util.geo.Geo()
    do_thing()
    print(config.DEBUG, os.getpid())
